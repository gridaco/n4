//! One in-process FFmpeg session per active decode cursor.
use super::*;
use std::ffi::{CString, c_char, c_int, c_void};
use std::ptr::NonNull;

unsafe extern "C" {
    fn n4_decoder_open(path: *const c_char, threads: c_int, out: *mut *mut c_void) -> c_int;
    #[cfg(test)]
    fn n4_decoder_retained(decoder: *const c_void) -> usize;
    fn n4_decoder_close(decoder: *mut c_void);
    fn n4_decoder_read(
        decoder: *mut c_void,
        pts: i64,
        seek_pts: i64,
        cache_capacity: usize,
        filters: *const c_char,
        dest: *mut u8,
        bytes: c_int,
    ) -> c_int;
    fn av_strerror(error: c_int, buffer: *mut c_char, size: usize) -> c_int;
}

pub(super) struct DecodeCursor {
    pub(super) key: Arc<PathBuf>,
    decoder: NonNull<c_void>,
    filters: CString,
    next: u32,
}

// FFmpeg sessions can move between threads. Calls and destruction require
// exclusive ownership; the session never keeps a pointer to Rust frame bytes.
unsafe impl Send for DecodeCursor {}

impl DecodeCursor {
    pub(super) fn is_near(&self, source: &IndexedVideo, ordinal: u32) -> bool {
        if !Arc::ptr_eq(&self.key, &source.key) {
            return false;
        }
        let keys = &source.index.keyframes;
        let current_gop = keys.partition_point(|&k| k < self.next);
        let target_gop = keys.partition_point(|&k| k <= ordinal);
        current_gop.abs_diff(target_gop) <= 1
    }
}

impl Drop for DecodeCursor {
    fn drop(&mut self) {
        unsafe { n4_decoder_close(self.decoder.as_ptr()) };
    }
}

fn av_result(result: c_int) -> Result<()> {
    if result < 0 {
        let mut error = [0_i8; 128];
        unsafe { av_strerror(result, error.as_mut_ptr(), error.len()) };
        let description = unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) };
        bail!("FFmpeg {} ({result})", description.to_string_lossy());
    }
    Ok(())
}

impl IndexedVideo {
    pub(super) fn decode(
        &self,
        ordinal: u32,
        preroll_capacity: usize,
        active: &mut Option<DecodeCursor>,
    ) -> Result<Vec<u8>> {
        let pts = *self
            .index
            .pts
            .get(ordinal as usize)
            .context("source ordinal out of bounds")?;
        let bytes: c_int = self
            .rgba_len
            .try_into()
            .context("FFmpeg output exceeds frame size limit")?;
        if active
            .as_ref()
            .is_none_or(|c| !Arc::ptr_eq(&c.key, &self.key))
        {
            *active = None;
            let path = CString::new(self.path.as_os_str().as_encoded_bytes())?;
            let filters = CString::new(self.filter.as_str())?;
            let mut raw = std::ptr::null_mut();
            av_result(unsafe {
                n4_decoder_open(path.as_ptr(), self.decode_threads.into(), &mut raw)
            })
            .with_context(|| format!("open video decoder for {}", self.path.display()))?;
            *active = Some(DecodeCursor {
                key: Arc::clone(&self.key),
                decoder: NonNull::new(raw).context("FFmpeg returned an empty decoder")?,
                filters,
                next: 0,
            });
        }
        let cursor = active.as_mut().context("missing video decoder")?;
        // Forward within the same GOP: consume intervening pictures in the codec.
        // Jump past a random-access point or reverse: demux seek and flush refs.
        let keys = &self.index.keyframes;
        let start = keys[..keys.partition_point(|&k| k <= ordinal)]
            .last()
            .copied()
            .unwrap_or(0);
        // With reordered packets, seeking backward from the target PTS can land at
        // the *next* GOP's decode timestamp. Start from this picture's own GOP.
        let seek_pts = if ordinal < cursor.next || start > cursor.next {
            self.index.pts[start as usize]
        } else {
            i64::MIN // AV_NOPTS_VALUE: retain the sequential cursor
        };
        let mut rgba = vec![0; self.rgba_len];
        let result = unsafe {
            n4_decoder_read(
                cursor.decoder.as_ptr(),
                pts,
                seek_pts,
                preroll_capacity,
                cursor.filters.as_ptr(),
                rgba.as_mut_ptr(),
                bytes,
            )
        };
        if let Err(error) = av_result(result) {
            *active = None;
            return Err(error).with_context(|| {
                format!(
                    "read {} at display frame {ordinal} (PTS {pts})",
                    self.path.display()
                )
            });
        }
        if result == 0 {
            cursor.next = ordinal + 1;
        }
        Ok(rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preroll_hits_keep_the_forward_cursor_and_evict_within_budget() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered-vfr.mp4");
        let video = IndexedVideo::open(&path, 320, 180, FrameRate(30, 1))?;
        let mut cursor = None;
        let budget = 2 * 1024 * 1024;
        video.decode(20, budget, &mut cursor)?;
        let next = |c: &Option<DecodeCursor>| c.as_ref().unwrap().next;
        assert_eq!(next(&cursor), 21);
        video.decode(18, budget, &mut cursor)?;
        assert_eq!(
            next(&cursor),
            21,
            "hit the saved preroll without repositioning"
        );
        video.decode(21, budget, &mut cursor)?;
        assert_eq!(next(&cursor), 22);
        video.decode(1, budget, &mut cursor)?;
        assert_eq!(next(&cursor), 2, "old preroll must have been evicted");
        video.decode(20, 1024, &mut cursor)?;
        assert_eq!(
            unsafe { n4_decoder_retained(cursor.as_ref().unwrap().decoder.as_ptr()) },
            0,
            "a reduced or too-small window must release the charged storage"
        );
        Ok(())
    }

    #[test]
    fn overlapping_profiles_keep_two_cursors_with_one_frame_cache_budget() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered-vfr.mp4");
        let primary = IndexedVideo::open(&path, 320, 180, FrameRate(30, 1))?;
        let inset = IndexedVideo::open(&path, 160, 90, FrameRate(30, 1))?;
        let mut reader = VideoReader::with_limits(4 * 1024 * 1024, 2 * 1024 * 1024);
        let barcode = |pixels: &[u8], width: usize| {
            (0..7).fold(0, |id, bit| {
                let x = (8 + 8 * bit) * width / 320;
                let y = 172 * width / 320;
                id | (u32::from(pixels[4 * (y * width + x)] > 128) << bit)
            })
        };

        reader.begin_frame([&primary]);
        reader.read(&primary, 0)?;
        reader.read(&primary, 16)?;
        let mut peak_preroll = 0;
        for (a, b) in [(20, 28), (12, 36), (17, 38), (32, 53), (27, 45), (18, 49)] {
            reader.begin_frame([&primary, &inset]);
            for (source, ordinal, width) in [(&primary, a, 320), (&inset, b, 160)] {
                let (pixels, hit) = reader.read(source, ordinal)?;
                assert!(!hit);
                assert_eq!(barcode(&pixels, width), ordinal);
                let retained: usize = reader
                    .cursors
                    .iter()
                    .map(|cursor| unsafe { n4_decoder_retained(cursor.decoder.decoder.as_ptr()) })
                    .sum();
                peak_preroll = peak_preroll.max(retained);
                assert!(retained <= reader.preroll_limit);
                assert!(reader.bytes + retained <= reader.limit);
            }
            assert_eq!(reader.cursors.len(), 2);
            for source in [&primary, &inset] {
                assert!(
                    reader
                        .cursors
                        .iter()
                        .any(|c| source.has_identity(&c.decoder.key))
                );
            }
        }
        assert!(peak_preroll > 0);

        reader.begin_frame([&primary]);
        assert_eq!(reader.cursors.len(), 1);
        assert!(primary.has_identity(&reader.cursors[0].decoder.key));
        let (picture, hit) = reader.read(&primary, 18)?;
        assert!(hit);
        assert_eq!(barcode(&picture, 320), 18);
        Ok(())
    }

    #[test]
    fn simultaneous_feeds_keep_their_sessions_and_share_bounded_preroll() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered-vfr.mp4");
        let profiles = (0..MAX_DECODE_CURSORS + 2)
            .map(|i| {
                IndexedVideo::open(
                    &path,
                    320 - 32 * i as u32,
                    180 - 18 * i as u32,
                    FrameRate(30, 1),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let mut reader = VideoReader::with_limits(4 * 1024 * 1024, 2 * 1024 * 1024);
        let session = |r: &VideoReader, p: &IndexedVideo| {
            r.cursors
                .iter()
                .find(|c| p.has_identity(&c.decoder.key))
                .map(|c| c.decoder.decoder)
        };
        // Multiple layers reading one profile should consume one slot.
        reader.begin_frame(profiles[..4].iter().chain(&profiles[..4]));
        assert_eq!(reader.active_profiles.len(), 4);
        for p in &profiles[..4] {
            reader.read(p, 0)?;
        }
        let first: Vec<_> = profiles[..4]
            .iter()
            .map(|p| session(&reader, p).unwrap())
            .collect();
        for ordinal in [1, 5, 8, 12] {
            reader.begin_frame(&profiles);
            for p in &profiles {
                reader.read(p, ordinal)?;
                assert!(reader.cursors.len() <= MAX_DECODE_CURSORS);
                let preroll: usize = reader
                    .cursors
                    .iter()
                    .map(|c| unsafe { n4_decoder_retained(c.decoder.decoder.as_ptr()) })
                    .sum();
                assert!(preroll <= reader.preroll_limit && reader.bytes + preroll <= reader.limit);
            }
            for (p, initial) in profiles.iter().zip(&first) {
                assert_eq!(session(&reader, p), Some(*initial));
            }
            assert_eq!(reader.cursors.len(), MAX_DECODE_CURSORS);
        }
        // A composition-only cut releases sessions immediately.
        reader.begin_frame([]);
        assert!(reader.cursors.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "manual exact-pixel check with an external 1080p SDR sample"]
    fn sample_random_access_matches_from_start_decode() -> Result<()> {
        let path = std::env::var_os("N4_MEDIA_SAMPLE").context("set N4_MEDIA_SAMPLE")?;
        let video = IndexedVideo::open(Path::new(&path), 1920, 1080, FrameRate(30, 1))?;
        let last = u32::try_from(video.index.pts.len() - 1)?;
        ensure!(last >= 30, "source must have at least 31 pictures");
        let targets: Vec<_> = (0..12).map(|i| (u64::from(last) * i / 11) as u32).collect();
        let select = targets
            .iter()
            .map(|n| format!("eq(n,{n})"))
            .collect::<Vec<_>>()
            .join("+");
        let filter = format!("select='{select}',{}", video.filter);
        let oracle = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-threads:v",
                "4",
                "-filter_threads",
                "3",
                "-i",
            ])
            .arg(&path)
            .args([
                "-map",
                "0:v:0",
                "-vf",
                &filter,
                "-fps_mode",
                "passthrough",
                "-threads:v",
                "1",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .output()?;
        ensure!(oracle.status.success(), "from-start oracle failed");
        ensure!(
            oracle.stdout.len() == targets.len() * video.rgba_len,
            "oracle frame count differs"
        );
        let mut reader = VideoReader::default();
        for i in (0..targets.len()).map(|i| i * 7 % targets.len()) {
            let (pixels, _) = reader.read(&video, targets[i])?;
            let start = i * video.rgba_len;
            ensure!(
                pixels.as_slice() == &oracle.stdout[start..start + video.rgba_len],
                "full-resolution RGBA mismatch at source ordinal {}",
                targets[i]
            );
        }
        println!("linear_cli_exact_1080p_ordinals={targets:?}");
        Ok(())
    }

    #[test]
    fn exact_seeks_match_linear_cli_pixels_through_eof_and_back() -> Result<()> {
        for (name, width, height) in [
            ("numbered-vfr.mp4", 320, 180),
            ("numbered-vfr.mp4", 640, 360),
            ("numbered.mkv", 160, 90),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join(name);
            let video = IndexedVideo::open(&path, width, height, FrameRate(30, 1))?;
            let oracle = Command::new("ffmpeg")
                .args(["-nostdin", "-v", "error", "-filter_threads", "3", "-i"])
                .arg(&path)
                .args([
                    "-map",
                    "0:v:0",
                    "-vf",
                    &video.filter,
                    "-fps_mode",
                    "passthrough",
                    "-pix_fmt",
                    "rgba",
                    "-threads:v",
                    "1",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ])
                .output()?;
            ensure!(oracle.status.success(), "linear reference failed");
            let count = video.index.pts.len() as u32;
            assert_eq!(oracle.stdout.len(), count as usize * video.rgba_len);
            let mut reader = VideoReader::with_limits(4 * 1024 * 1024, 2 * 1024 * 1024);
            let mut seed = 6842_u32;
            let random_targets = (0..90).map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                seed % count
            });
            for i in [
                count - 1,
                0,
                3,
                2,
                count / 2,
                count / 2 + 4,
                count - 2,
                count - 1,
                0,
            ]
            .into_iter()
            .chain(random_targets)
            {
                let (pixels, _) = reader.read(&video, i)?;
                let cursor = &reader.cursors.back().unwrap().decoder;
                assert!(
                    unsafe { n4_decoder_retained(cursor.decoder.as_ptr()) } + reader.bytes
                        <= reader.limit
                );
                let start = i as usize * video.rgba_len;
                assert!(
                    pixels.as_slice() == &oracle.stdout[start..start + video.rgba_len],
                    "{name} display ordinal {i} differs from the CLI's sequential decode"
                );
            }
        }
        Ok(())
    }
}
