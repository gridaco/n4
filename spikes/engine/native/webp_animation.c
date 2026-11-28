// Use the installed header's versioned constructor and verify the Rust buffer dimensions.
#include <webp/demux.h>

WebPAnimDecoder* n4_webp_animation_open(const uint8_t* bytes, size_t length,
                                        uint32_t width, uint32_t height,
                                        uint32_t frames) {
    WebPData data = {bytes, length};
    WebPAnimDecoder* decoder = WebPAnimDecoderNew(&data, NULL);  // straight RGBA
    WebPAnimInfo info;
    if (!decoder) return NULL;
    if (!WebPAnimDecoderGetInfo(decoder, &info) || info.canvas_width != width ||
        info.canvas_height != height || info.frame_count != frames) {
        WebPAnimDecoderDelete(decoder);
        return NULL;
    }
    return decoder;
}
