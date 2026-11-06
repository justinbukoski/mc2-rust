// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! MPEG-1 cut-scene playback using the pure-Rust `mc2-movie` decoder.
//!
//! Movies may be opened by path or by a VFS name. Frames retain their MPEG presentation
//! timestamps and audio is queued slightly ahead to a [`QueueWriter`] for the app mixer.

use crate::audio::mixer::QueueWriter;
use mc2_formats::vfs::Vfs;
use mc2_movie::demux::{self, ElementaryStream};
use mc2_movie::{AudioDecoder, AudioPcm, MovieError as DecoderError, VideoDecoder};
use std::path::Path;

#[derive(Debug)]
pub enum MovieError {
    Io(std::io::Error),
    Vfs(String),
    Missing(String),
    NotMpeg,
    Decode(DecoderError),
}

impl std::fmt::Display for MovieError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MovieError::Io(e) => write!(f, "movie I/O error: {e}"),
            MovieError::Vfs(e) => write!(f, "movie VFS error: {e}"),
            MovieError::Missing(name) => write!(f, "movie not found: {name}"),
            MovieError::NotMpeg => write!(f, "not an MPEG-1 program stream"),
            MovieError::Decode(e) => write!(f, "movie decode error: {e}"),
        }
    }
}

impl std::error::Error for MovieError {}

impl From<DecoderError> for MovieError {
    fn from(value: DecoderError) -> Self {
        match value {
            DecoderError::NotMpeg => Self::NotMpeg,
            other => Self::Decode(other),
        }
    }
}

/// Stream properties.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovieInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    /// Seconds from the decoded video picture count, or zero if unavailable.
    pub duration: f64,
    pub has_audio: bool,
    /// Audio sample rate (zero without audio). Audio is delivered as stereo.
    pub audio_sample_rate: u32,
}

/// One decoded video frame: planar YCbCr 4:2:0.
pub type VideoFrame = mc2_movie::Frame;

/// A chunk of decoded audio: one or more MPEG audio frames, interleaved stereo.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioChunk {
    /// Presentation time for the first sample.
    pub time: f64,
    /// Interleaved left/right samples, normalized to -1..1.
    pub samples: Vec<f32>,
}

/// An opened movie stream.
pub struct Movie {
    video_stream: ElementaryStream,
    video: VideoDecoder,
    pending_video: Option<VideoFrame>,
    pts_origin: f64,
    audio: Option<AudioPcm>,
    audio_offset: usize,
    info: MovieInfo,
    video_enabled: bool,
    audio_enabled: bool,
    video_done: bool,
}

impl Movie {
    /// Open an MPEG movie from a filesystem path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MovieError> {
        Self::from_bytes(std::fs::read(path).map_err(MovieError::Io)?)
    }

    /// Open an MPEG movie by its mounted VFS name (for example `data/movies/msft.mpg`).
    pub fn open_vfs(vfs: &mut Vfs, name: &str) -> Result<Self, MovieError> {
        let bytes = vfs
            .read(name)
            .map_err(|e| MovieError::Vfs(e.to_string()))?
            .ok_or_else(|| MovieError::Missing(name.to_owned()))?;
        Self::from_bytes(bytes)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self, MovieError> {
        let streams = demux::demux(&data)?;
        let video_stream = streams.video.ok_or(DecoderError::NotMpeg)?;
        Self::from_streams(video_stream, streams.audio)
    }

    /// Build playback from already demultiplexed MPEG elementary streams.
    pub fn from_streams(
        video_stream: ElementaryStream,
        audio_stream: Option<ElementaryStream>,
    ) -> Result<Self, MovieError> {
        let mut video = VideoDecoder::new(video_stream.data.clone(), video_stream.marks.clone());
        video.set_strict(true);
        let sequence = video.read_headers()?;
        let frame_count = video.count_pictures();
        let audio = audio_stream
            .map(AudioDecoder::from_stream)
            .map(|d| d.decode())
            .transpose()?;
        let mut pending_video = video.next_frame()?.ok_or(DecoderError::NotMpeg)?;
        // MPEG PTS values may begin well after zero. Start the player's clock at the first
        // presented video frame and express both streams relative to that same origin.
        let pts_origin = pending_video.time;
        pending_video.time -= pts_origin;
        let (has_audio, audio_sample_rate) =
            audio.as_ref().map_or((false, 0), |a| (true, a.sample_rate));
        let info = MovieInfo {
            width: sequence.width,
            height: sequence.height,
            frame_rate: sequence.frame_rate,
            duration: if sequence.frame_rate > 0.0 {
                frame_count as f64 / sequence.frame_rate
            } else {
                0.0
            },
            has_audio,
            audio_sample_rate,
        };
        Ok(Self {
            video_stream,
            video,
            pending_video: Some(pending_video),
            pts_origin,
            audio,
            audio_offset: 0,
            info,
            video_enabled: true,
            audio_enabled: true,
            video_done: false,
        })
    }

    pub fn info(&self) -> MovieInfo {
        self.info
    }

    pub fn set_audio_enabled(&mut self, on: bool) {
        self.audio_enabled = on;
    }

    pub fn set_video_enabled(&mut self, on: bool) {
        self.video_enabled = on;
    }

    /// Decode the next frame in presentation order; `None` at end of video.
    pub fn next_video_frame(&mut self) -> Option<VideoFrame> {
        self.try_next_video_frame().ok().flatten()
    }

    /// Fallible form of [`Movie::next_video_frame`], used by production playback so a damaged
    /// later picture cannot be mistaken for the end of a movie.
    pub fn try_next_video_frame(&mut self) -> Result<Option<VideoFrame>, MovieError> {
        if !self.video_enabled || self.video_done {
            return Ok(None);
        }
        if let Some(frame) = self.pending_video.take() {
            return Ok(Some(frame));
        }
        match self.video.next_frame().map_err(MovieError::from) {
            Ok(Some(mut frame)) => {
                frame.time -= self.pts_origin;
                Ok(Some(frame))
            }
            Ok(None) => {
                self.video_done = true;
                Ok(None)
            }
            Err(error) => {
                self.video_done = true;
                Err(error)
            }
        }
    }

    /// Return up to 1152 stereo sample frames and preserve the stream PTS.
    pub fn next_audio_chunk(&mut self) -> Option<AudioChunk> {
        if !self.audio_enabled {
            return None;
        }
        let audio = self.audio.as_ref()?;
        let channels = audio.channels;
        let sample_frames = audio.samples.len() / channels;
        if self.audio_offset >= sample_frames {
            return None;
        }
        let count = (sample_frames - self.audio_offset).min(1152);
        let from = self.audio_offset * channels;
        let to = from + count * channels;
        let mut samples = Vec::with_capacity(count * 2);
        if channels == 1 {
            for &sample in &audio.samples[from..to] {
                samples.extend_from_slice(&[sample, sample]);
            }
        } else {
            samples.extend_from_slice(&audio.samples[from..to]);
        }
        let time = audio.pts.map_or(0.0, |pts| pts - self.pts_origin)
            + self.audio_offset as f64 / audio.sample_rate as f64;
        self.audio_offset += count;
        Some(AudioChunk { time, samples })
    }

    pub fn has_ended(&self) -> bool {
        (!self.video_enabled || self.video_done)
            && (!self.audio_enabled
                || self
                    .audio
                    .as_ref()
                    .is_none_or(|a| self.audio_offset >= a.samples.len() / a.channels))
    }

    /// Restart from the first frame and audio sample.
    pub fn rewind(&mut self) {
        self.video = VideoDecoder::new(
            self.video_stream.data.clone(),
            self.video_stream.marks.clone(),
        );
        self.video.set_strict(true);
        self.pending_video = None;
        if let Err(e) = self.video.read_headers() {
            log::error!("movie rewind failed to read MPEG headers: {e}");
            self.video_done = true;
        } else {
            self.pending_video = self.video.next_frame().ok().flatten().map(|mut frame| {
                frame.time -= self.pts_origin;
                frame
            });
            self.video_done = self.pending_video.is_none();
        }
        self.audio_offset = 0;
    }
}

/// Plays a movie against a monotonic frame clock and feeds PCM to the app mixer.
pub struct MoviePlayer {
    movie: Movie,
    clock: f64,
    audio: Option<QueueWriter>,
    audio_time: f64,
    pending_frame: Option<VideoFrame>,
    video_done: bool,
    audio_done: bool,
    failure: Option<MovieError>,
}

const AUDIO_LEAD: f64 = 0.25;

/// Append one timed chunk to the queue, padding a PTS gap and trimming samples that precede the
/// current audio cursor. Returns the chunk's end time.
fn queue_audio_chunk(writer: &QueueWriter, cursor: f64, chunk: &AudioChunk, rate: u32) -> f64 {
    let rate = f64::from(rate);
    let chunk_frames = chunk.samples.len() / 2;
    let chunk_end = chunk.time + chunk_frames as f64 / rate;
    let overlap = (cursor - chunk.time).max(0.0);
    let skip_frames = (overlap * rate).round() as usize;
    let audible_start = chunk.time + skip_frames as f64 / rate;
    let gap_frames = ((audible_start - cursor).max(0.0) * rate).round() as usize;
    if gap_frames > 0 {
        writer.push(&vec![0.0; gap_frames.saturating_mul(2)]);
    }
    if skip_frames < chunk_frames {
        writer.push(&chunk.samples[skip_frames * 2..]);
    }
    cursor.max(chunk_end)
}

impl MoviePlayer {
    /// The writer receives interleaved stereo at the movie's decoded sample rate.
    pub fn new(mut movie: Movie, audio: Option<QueueWriter>) -> Self {
        let has_audio = movie.info.has_audio && audio.is_some();
        movie.set_audio_enabled(has_audio);
        let mut player = Self {
            movie,
            clock: 0.0,
            audio,
            audio_time: 0.0,
            pending_frame: None,
            video_done: false,
            audio_done: !has_audio,
            failure: None,
        };
        match player.movie.try_next_video_frame() {
            Ok(frame) => player.pending_frame = frame,
            Err(error) => {
                player.failure = Some(error);
                player.video_done = true;
            }
        }
        player.video_done = player.pending_frame.is_none();
        player
    }

    pub fn info(&self) -> MovieInfo {
        self.movie.info
    }

    pub fn clock(&self) -> f64 {
        self.clock
    }

    /// Stereo sample frames still waiting in the app mixer queue.
    pub fn queued_audio_frames(&self) -> usize {
        self.audio.as_ref().map_or(0, QueueWriter::queued_frames)
    }

    pub fn is_finished(&self) -> bool {
        self.video_done
            && self.pending_frame.is_none()
            && self.audio_done
            && self.audio.as_ref().is_none_or(QueueWriter::drained)
    }

    /// Take a decode failure reported while advancing playback.
    pub fn take_error(&mut self) -> Option<MovieError> {
        self.failure.take()
    }

    /// Advance playback; returns the newest frame due at its PTS, dropping overdue frames.
    pub fn advance(&mut self, dt: f64) -> Option<VideoFrame> {
        self.clock += dt.max(0.0);
        let mut latest = None;
        while self
            .pending_frame
            .as_ref()
            .is_some_and(|f| f.time <= self.clock)
        {
            latest = self.pending_frame.take();
            match self.movie.try_next_video_frame() {
                Ok(frame) => {
                    self.pending_frame = frame;
                    if self.pending_frame.is_none() {
                        self.video_done = true;
                    }
                }
                Err(error) => {
                    self.failure = Some(error);
                    self.pending_frame = None;
                    self.video_done = true;
                    self.audio_done = true;
                    if let Some(writer) = &self.audio {
                        writer.cancel();
                    }
                }
            }
        }
        self.pump_audio();
        latest
    }

    /// Stop video and audio immediately, matching a user skip.
    pub fn skip(&mut self) {
        self.pending_frame = None;
        self.video_done = true;
        self.audio_done = true;
        if let Some(writer) = &self.audio {
            writer.cancel();
        }
    }

    fn pump_audio(&mut self) {
        let Some(writer) = self.audio.as_ref() else {
            return;
        };
        while !self.audio_done && self.audio_time < self.clock + AUDIO_LEAD {
            match self.movie.next_audio_chunk() {
                Some(chunk) => {
                    self.audio_time = queue_audio_chunk(
                        writer,
                        self.audio_time,
                        &chunk,
                        self.movie.info.audio_sample_rate,
                    );
                }
                None => {
                    self.audio_done = true;
                    writer.finish();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_decode_error_after_a_valid_first_picture() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("skipping later-picture failure evidence: MC2_DATA is unset");
            return;
        };
        let path = Path::new(&root).join("data/movies/msft.mpg");
        let movie = Movie::open(path).expect("open intro movie");
        let mut player = MoviePlayer::new(movie, None);

        // Keep the real first frame queued for display, then make the unread decoder's first
        // picture type invalid. Strict production decoding must return an error, not EOF.
        let data = &mut player.movie.video_stream.data;
        let picture = data
            .windows(4)
            .position(|w| w == [0, 0, 1, 0])
            .expect("intro picture start code");
        data[picture + 5] &= !0x38; // picture_coding_type = 000 (reserved)
        player.movie.video = VideoDecoder::new(data.clone(), Vec::new());
        player.movie.video.set_strict(true);
        player
            .movie
            .video
            .read_headers()
            .expect("intro sequence header");
        assert!(
            player.advance(0.001).is_some(),
            "first valid frame is displayed"
        );
        assert!(matches!(player.take_error(), Some(MovieError::Decode(_))));
    }

    #[test]
    fn audio_pts_over_40ms_is_scheduled_in_the_mixer_output() {
        use crate::audio::mixer::{
            Mixer, PlayMode, QueueSource, APP_MIXER_CHANNELS, MOVIE_AUDIO_CHANNEL,
        };
        let sample_rate = 44_100u32;
        let delay = 0.120;
        let samples = (0..1152)
            .flat_map(|i| {
                let sample = if i < 8 {
                    0.0
                } else {
                    (i as f32 * 0.23).sin() * 0.5
                };
                [sample, sample]
            })
            .collect();
        let chunk = AudioChunk {
            time: delay,
            samples,
        };

        let (source, writer) = QueueSource::new(2, sample_rate);
        let mut mixer = Mixer::new(sample_rate, APP_MIXER_CHANNELS);
        mixer.assign(MOVIE_AUDIO_CHANNEL, Box::new(source));
        mixer.set_mode(MOVIE_AUDIO_CHANNEL, PlayMode::PlayOnce);
        queue_audio_chunk(&writer, 0.0, &chunk, sample_rate);
        let mut output = vec![0.0; sample_rate as usize * 2 / 5];
        mixer.render(&mut output);
        let emitted = output
            .as_chunks::<2>()
            .0
            .iter()
            .position(|frame| frame.iter().any(|sample| sample.abs() > 0.0001))
            .expect("scheduled PCM reaches mixer output");
        let expected = (delay * f64::from(sample_rate)).round() as usize + 8;
        let tolerance = (f64::from(sample_rate) * 0.040).round() as usize;
        assert!(
            emitted.abs_diff(expected) <= tolerance,
            "first identifiable PCM emitted at {emitted} samples; PTS predicts {expected} (40 ms tolerance {tolerance})"
        );
    }
}
