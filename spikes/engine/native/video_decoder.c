// A single worker owns this decoder, its demuxer and its color conversion graph.
// Only requested presentation timestamps pass through the filters.
#include <errno.h>
#include <stdint.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavfilter/avfilter.h>
#include <libavfilter/buffersink.h>
#include <libavfilter/buffersrc.h>
#include <libavformat/avformat.h>
#include <libavutil/imgutils.h>

#define PREROLL_SLOTS 64

typedef struct {
    AVFrame *frame;
    size_t bytes;
} SavedPicture;

typedef struct {
    AVFormatContext *file;
    AVCodecContext *codec;
    int stream;
    AVPacket *packet;
    AVFrame *picture;
    AVFrame *output;
    AVFilterGraph *graph;
    AVFilterContext *source;
    AVFilterContext *sink;
    SavedPicture saved[PREROLL_SLOTS];
    size_t oldest, count, retained, capacity;
} N4Decoder;

static size_t slot_at(const N4Decoder *d, size_t i) {
    return (d->oldest + i) % PREROLL_SLOTS;
}

static AVFrame *saved_picture(const N4Decoder *d, int64_t pts) {
    for (size_t i = 0; i < d->count; ++i) {
        AVFrame *frame = d->saved[slot_at(d, i)].frame;
        if (frame->pts == pts) return frame;
    }
    return NULL;
}

static void drop_oldest(N4Decoder *d) {
    SavedPicture *entry = &d->saved[d->oldest];
    d->retained -= entry->bytes;
    av_frame_free(&entry->frame);
    d->oldest = slot_at(d, 1);
    --d->count;
}

// Cache only pictures that the codec already had to decode on its way to the
// request. Count the sizes of referenced storage, including stride padding.
static void remember_preroll(N4Decoder *d, AVFrame *picture) {
    if (!d->capacity || saved_picture(d, picture->pts)) return;
    size_t bytes = sizeof(*picture);
    for (size_t i = 0; i < AV_NUM_DATA_POINTERS; ++i)
        if (picture->buf[i]) bytes += picture->buf[i]->size;
    for (int i = 0; i < picture->nb_extended_buf; ++i) bytes += picture->extended_buf[i]->size;
    for (int i = 0; i < picture->nb_side_data; ++i) bytes += picture->side_data[i]->buf->size;
    if (bytes > d->capacity) return;
    while (d->count == PREROLL_SLOTS || d->retained > d->capacity - bytes) drop_oldest(d);
    SavedPicture *entry = &d->saved[slot_at(d, d->count)];
    entry->frame = av_frame_clone(picture);
    if (!entry->frame) return;
    entry->bytes = bytes;
    d->retained += bytes;
    ++d->count;
}

size_t n4_decoder_retained(const N4Decoder *d) { return d->retained; }

void n4_decoder_close(N4Decoder *d) {
    if (!d) return;
    while (d->count) drop_oldest(d);
    avfilter_graph_free(&d->graph);
    av_frame_free(&d->output);
    av_frame_free(&d->picture);
    av_packet_free(&d->packet);
    avcodec_free_context(&d->codec);
    avformat_close_input(&d->file);
    av_free(d);
}

int n4_decoder_open(const char *path, int threads, N4Decoder **out) {
    N4Decoder *d = av_mallocz(sizeof(*d));
    if (!d) return AVERROR(ENOMEM);
    int ret = avformat_open_input(&d->file, path, NULL, NULL);
    if (ret < 0) goto fail;
    ret = avformat_find_stream_info(d->file, NULL);
    if (ret < 0) goto fail;
    // ffprobe indexes v:0. A "best" stream could select another video track.
    unsigned i;
    for (i = 0; i < d->file->nb_streams; ++i)
        if (d->file->streams[i]->codecpar->codec_type == AVMEDIA_TYPE_VIDEO) break;
    if (i == d->file->nb_streams) { ret = AVERROR_STREAM_NOT_FOUND; goto fail; }
    d->stream = i;
    AVStream *stream = d->file->streams[i];
    const AVCodec *decoder = avcodec_find_decoder(stream->codecpar->codec_id);
    if (!decoder) { ret = AVERROR_DECODER_NOT_FOUND; goto fail; }
    d->codec = avcodec_alloc_context3(decoder);
    d->packet = av_packet_alloc();
    d->picture = av_frame_alloc();
    d->output = av_frame_alloc();
    if (!d->codec || !d->packet || !d->picture || !d->output) { ret = AVERROR(ENOMEM); goto fail; }
    ret = avcodec_parameters_to_context(d->codec, stream->codecpar);
    if (ret < 0) goto fail;
    d->codec->pkt_timebase = stream->time_base;
    d->codec->thread_count = threads;
    ret = avcodec_open2(d->codec, decoder, NULL);
    if (ret < 0) goto fail;
    *out = d;
    return 0;
fail:
    n4_decoder_close(d);
    return ret;
}

static int filter_open(N4Decoder *d, const char *filters) {
    d->graph = avfilter_graph_alloc();
    if (!d->graph) return AVERROR(ENOMEM);
    d->graph->nb_threads = 3;
    d->source = avfilter_graph_alloc_filter(d->graph, avfilter_get_by_name("buffer"), "source");
    if (!d->source) return AVERROR(ENOMEM);
    AVBufferSrcParameters *params = av_buffersrc_parameters_alloc();
    if (!params) return AVERROR(ENOMEM);
    params->time_base = d->file->streams[d->stream]->time_base;
    params->width = d->picture->width;
    params->height = d->picture->height;
    params->format = d->picture->format;
    params->sample_aspect_ratio = d->picture->sample_aspect_ratio;
    params->color_range = d->picture->color_range;
    params->color_space = d->picture->colorspace;
    int ret = av_buffersrc_parameters_set(d->source, params);
    av_free(params);
    if (ret < 0) return ret;
    ret = avfilter_init_str(d->source, NULL);
    if (ret < 0) return ret;
    ret = avfilter_graph_create_filter(&d->sink, avfilter_get_by_name("buffersink"),
                                       "sink", NULL, NULL, d->graph);
    if (ret < 0) return ret;
    AVFilterInOut *in = avfilter_inout_alloc(), *out = avfilter_inout_alloc();
    ret = AVERROR(ENOMEM);
    if (!in || !out) goto done;
    in->name = av_strdup("out");
    in->filter_ctx = d->sink;
    out->name = av_strdup("in");
    out->filter_ctx = d->source;
    if (!in->name || !out->name) goto done;
    ret = avfilter_graph_parse_ptr(d->graph, filters, &in, &out, NULL);
    if (ret >= 0) ret = avfilter_graph_config(d->graph, NULL);
done:
    avfilter_inout_free(&in);
    avfilter_inout_free(&out);
    return ret;
}

static int next_picture(N4Decoder *d) {
    for (;;) {
        int ret = avcodec_receive_frame(d->codec, d->picture);
        if (ret != AVERROR(EAGAIN)) return ret;
        do {
            av_packet_unref(d->packet);
            ret = av_read_frame(d->file, d->packet);
        } while (ret >= 0 && d->packet->stream_index != d->stream);
        if (ret < 0 && ret != AVERROR_EOF) return ret;
        ret = avcodec_send_packet(d->codec, ret == AVERROR_EOF ? NULL : d->packet);
        av_packet_unref(d->packet);
        if (ret < 0) return ret;
    }
}

int n4_decoder_read(N4Decoder *d, int64_t target, int64_t seek_pts, size_t capacity, const char *filters,
                    uint8_t *dst, int bytes) {
    d->capacity = capacity;
    while (d->retained > capacity) drop_oldest(d);
    AVFrame *saved = saved_picture(d, target);
    if (!saved && seek_pts != AV_NOPTS_VALUE) {
        int ret = av_seek_frame(d->file, d->stream, seek_pts, AVSEEK_FLAG_BACKWARD);
        if (ret < 0) return ret;
        avcodec_flush_buffers(d->codec);
    }
    while (!saved) {
        int ret = next_picture(d);
        if (ret < 0) return ret;
        // Only accept a display PTS from the same clock as the ffprobe index.
        int64_t pts = d->picture->pts;
        if (pts == AV_NOPTS_VALUE || pts > target) return AVERROR_INVALIDDATA;
        if (pts == target) break;
        remember_preroll(d, d->picture);
    }
    if (!d->graph) {
        int ret = filter_open(d, filters);
        if (ret < 0) return ret;
    }
    int ret = av_buffersrc_add_frame_flags(d->source, saved ? saved : d->picture,
                                          saved ? AV_BUFFERSRC_FLAG_KEEP_REF : 0);
    if (ret < 0) return ret;
    ret = av_buffersink_get_frame(d->sink, d->output);
    if (ret < 0) return ret;
    if (d->output->format != AV_PIX_FMT_RGBA) ret = AVERROR_INVALIDDATA;
    else ret = av_image_copy_to_buffer(dst, bytes, (const uint8_t * const*)d->output->data,
                                       d->output->linesize, AV_PIX_FMT_RGBA,
                                       d->output->width, d->output->height, 1);
    av_frame_unref(d->output);
    return ret == bytes ? (saved != NULL) : (ret < 0 ? ret : AVERROR_INVALIDDATA);
}
