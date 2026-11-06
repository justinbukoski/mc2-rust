//! Background-decoded streaming source.
//!
//! Music and voice-over files are read and decoded on a worker thread and handed to the audio
//! callback through preallocated bounded queues, so the callback never does file I/O, takes a
//! lock held by another thread, or allocates: it only copies from the block it already holds and
//! swaps in the next one with `try_recv`. Used blocks go back to the worker through a second
//! bounded queue for reuse. On underrun the source plays one frame of silence at a time.
//!
//! **Gapless restarts.** The consumer keeps a copy of the stream's first block (`head`). A
//! rewind (replay, or the loop wrap after an end marker) switches to playing that copy at once
//! while the worker rewinds the source, discards the frames the head already covers and refills
//! the queue behind it, so the restart is audible immediately and continuous. When the channel
//! loops from the start, the worker also wraps by itself inside its blocks, with no marker at
//! all.

use super::mixer::{sealed, AudioSource, RealtimeSource, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::Duration;

/// Frames decoded per block.
const BLOCK_FRAMES: usize = 4096;
/// Blocks buffered ahead of playback (about 0.4 s at 44.1 kHz).
const QUEUE_BLOCKS: usize = 4;

struct Block {
    epoch: u64,
    /// Interleaved samples; empty marks the end of the stream for this epoch.
    samples: Vec<f32>,
}

struct Control {
    /// Epoch the consumer wants; a change makes the worker rewind the source.
    wanted_epoch: AtomicU64,
    /// Frames the worker must discard after rewinding (the consumer replays them from `head`).
    skip_frames: AtomicUsize,
    shutdown: AtomicBool,
    /// The channel is looping: the worker wraps around by itself, with no gap in the queue.
    looping: AtomicBool,
    /// The source's `rewind` failed on the worker; the stream ends after the cached head.
    rewind_failed: AtomicBool,
}

pub struct StreamingSource {
    channels: u16,
    sample_rate: u32,
    rx: Receiver<Block>,
    /// The worker's copy of the first block, sent just before that block itself.
    head_rx: Receiver<Vec<f32>>,
    recycle: SyncSender<Vec<f32>>,
    control: Arc<Control>,
    can_rewind: bool,
    epoch: u64,
    /// First block of the stream, kept for instant restarts.
    head: Vec<f32>,
    head_set: bool,
    /// Currently playing from `head` rather than `cur`.
    in_head: bool,
    cur: Vec<f32>,
    pos: usize,
    eof: bool,
    /// Samples of the current epoch have been handed out (a rewind is then a real restart).
    consumed: bool,
}

impl StreamingSource {
    /// Move `source` onto a worker thread. Waits briefly (game thread) for the first block so
    /// playback can start without an underrun.
    ///
    /// The source's sample rate is clamped into the supported range and its channel count must
    /// be 1 or 2.
    pub fn spawn(mut source: Box<dyn AudioSource>) -> Self {
        let channels = source.channels();
        assert!(
            channels == 1 || channels == 2,
            "unsupported channel count {channels}"
        );
        let sample_rate = source.sample_rate().clamp(MIN_SAMPLE_RATE, MAX_SAMPLE_RATE);
        let can_rewind = source.can_rewind();
        let (tx, rx) = sync_channel::<Block>(QUEUE_BLOCKS);
        let (recycle, recycled) = sync_channel::<Vec<f32>>(QUEUE_BLOCKS + 2);
        let (head_tx, head_rx) = sync_channel::<Vec<f32>>(1);
        let control = Arc::new(Control {
            wanted_epoch: AtomicU64::new(0),
            skip_frames: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            rewind_failed: AtomicBool::new(false),
        });
        let ctl = Arc::clone(&control);
        std::thread::Builder::new()
            .name("mc2-audio-stream".into())
            .spawn(move || {
                let ch = channels as usize;
                let mut epoch = 0u64;
                let mut at_eof = false;
                let mut pending: Option<Block> = None;
                let mut just_rewound = false;
                let mut head_sent = false;
                let mut scratch = vec![0.0f32; BLOCK_FRAMES * ch];
                while !ctl.shutdown.load(Ordering::Relaxed) {
                    let wanted = ctl.wanted_epoch.load(Ordering::Acquire);
                    if wanted != epoch {
                        epoch = wanted;
                        at_eof = false;
                        just_rewound = false;
                        pending = None;
                        if source.rewind() {
                            // The consumer replays the cached head: continue after it.
                            let mut left = ctl.skip_frames.load(Ordering::Acquire);
                            while left > 0 {
                                let want = left.min(BLOCK_FRAMES);
                                let n = source.read(&mut scratch[..want * ch]);
                                if n == 0 {
                                    break;
                                }
                                left -= n;
                            }
                        } else {
                            // Cannot restart: end the stream after the cached head.
                            ctl.rewind_failed.store(true, Ordering::Release);
                            at_eof = true;
                            pending = Some(Block {
                                epoch,
                                samples: Vec::new(),
                            });
                        }
                    }
                    let looping = can_rewind && ctl.looping.load(Ordering::Relaxed);
                    while pending.is_none() && !at_eof {
                        let mut buf = recycled.try_recv().unwrap_or_default();
                        buf.resize(BLOCK_FRAMES * ch, 0.0);
                        let mut frames = 0;
                        while frames < BLOCK_FRAMES {
                            let n = source.read(&mut buf[frames * ch..]);
                            if n > 0 {
                                frames += n;
                                just_rewound = false;
                            } else if looping && !just_rewound {
                                // End of the file while looping: wrap inside this block.
                                if source.rewind() {
                                    just_rewound = true;
                                } else {
                                    ctl.rewind_failed.store(true, Ordering::Release);
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                        buf.truncate(frames * ch);
                        if !head_sent {
                            // The consumer's restart copy of the stream start is made here, on
                            // the worker, and handed over ahead of the block itself.
                            head_sent = true;
                            let _ = head_tx.try_send(buf.clone());
                        }
                        if frames == 0 {
                            at_eof = true;
                        }
                        pending = Some(Block {
                            epoch,
                            samples: buf,
                        });
                    }
                    match pending.take() {
                        Some(b) => match tx.try_send(b) {
                            Ok(()) => {}
                            Err(TrySendError::Full(b)) => {
                                pending = Some(b);
                                std::thread::sleep(Duration::from_millis(2));
                            }
                            Err(TrySendError::Disconnected(_)) => return,
                        },
                        None => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
            .expect("spawn audio stream thread");
        let mut s = Self {
            channels,
            sample_rate,
            rx,
            head_rx,
            recycle,
            control,
            can_rewind,
            epoch: 0,
            head: Vec::new(),
            head_set: false,
            in_head: false,
            cur: Vec::new(),
            pos: 0,
            eof: false,
            consumed: false,
        };
        if let Ok(b) = s.rx.recv_timeout(Duration::from_millis(500)) {
            s.accept(b);
        }
        s
    }

    /// True if the worker found that the underlying source could not be rewound; the replay
    /// then ends after the cached first block.
    pub fn rewind_failed(&self) -> bool {
        self.control.rewind_failed.load(Ordering::Acquire)
    }

    fn accept(&mut self, b: Block) {
        if b.samples.is_empty() {
            self.eof = true;
        }
        if !self.head_set {
            // The worker queued its copy of the first block before the block itself; taking it
            // is a move, so nothing allocates here (this runs on the audio callback too).
            if let Ok(h) = self.head_rx.try_recv() {
                self.head = h;
                self.head_set = true;
            }
        }
        let old = std::mem::replace(&mut self.cur, b.samples);
        self.pos = 0;
        // Hand the spent buffer back; if the queue is full it is simply dropped.
        let _ = self.recycle.try_send(old);
    }
}

impl sealed::Sealed for StreamingSource {}
impl RealtimeSource for StreamingSource {}

impl AudioSource for StreamingSource {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn read(&mut self, out: &mut [f32]) -> usize {
        let ch = self.channels as usize;
        loop {
            let src: &[f32] = if self.in_head { &self.head } else { &self.cur };
            if self.pos < src.len() {
                let n = (src.len() - self.pos).min(out.len() / ch * ch);
                out[..n].copy_from_slice(&src[self.pos..self.pos + n]);
                self.pos += n;
                self.consumed = true;
                return n / ch;
            }
            if self.in_head {
                // The cached head is spent: carry on with the worker's blocks, which were
                // restarted behind it.
                self.in_head = false;
                self.pos = 0;
                self.cur.clear();
                continue;
            }
            if self.eof {
                return 0;
            }
            match self.rx.try_recv() {
                Ok(b) if b.epoch != self.epoch => {
                    let _ = self.recycle.try_send(b.samples);
                }
                Ok(b) => self.accept(b),
                Err(TryRecvError::Empty) => {
                    out[..ch].fill(0.0);
                    return 1;
                }
                Err(TryRecvError::Disconnected) => return 0,
            }
        }
    }

    fn rewind(&mut self) -> bool {
        if !self.can_rewind() {
            return false;
        }
        if !self.consumed {
            // Nothing played yet in this epoch: we are already at the start.
            return true;
        }
        self.epoch += 1;
        self.eof = false;
        self.consumed = false;
        self.cur.clear();
        // Play the cached head immediately; the worker discards what it covers and refills.
        self.in_head = true;
        self.pos = 0;
        self.control
            .skip_frames
            .store(self.head.len() / self.channels as usize, Ordering::Release);
        self.control
            .wanted_epoch
            .store(self.epoch, Ordering::Release);
        true
    }

    fn can_rewind(&self) -> bool {
        // Once the worker has found the source cannot really rewind, say so: a looping channel
        // then ends the stream instead of replaying the cached head forever.
        self.can_rewind && !self.rewind_failed()
    }

    fn set_looping(&mut self, looping: bool) {
        self.control.looping.store(looping, Ordering::Relaxed);
    }
}

impl Drop for StreamingSource {
    fn drop(&mut self) {
        // The worker notices on its next iteration (or when the queue disconnects) and exits.
        self.control.shutdown.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::mixer::{Mixer, PlayMode};
    use crate::audio::wav::testutil::pcm16_wav;
    use crate::audio::wav::WavStream;
    use std::io::Cursor;

    const RATE: u32 = 8000;
    /// Frames per paced step; each step sleeps for exactly its real-time duration.
    const STEP: usize = 256;

    /// Non-zero test signal so any underrun silence is visible as a mismatch.
    fn signal(n: usize) -> Vec<i16> {
        (0..n).map(|i| (i % 3000) as i16 + 1).collect()
    }

    fn source(n: usize) -> Box<dyn AudioSource> {
        let w = pcm16_wav(1, RATE, &signal(n));
        Box::new(WavStream::new(Cursor::new(w)).unwrap())
    }

    fn pace() {
        std::thread::sleep(Duration::from_secs_f64(STEP as f64 / RATE as f64));
    }

    /// Render `frames` mono-left samples at real-time pace, keeping every sample (silence
    /// included): a gap anywhere makes the comparison with the expected signal fail.
    fn collect(m: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut got = Vec::with_capacity(frames);
        let mut buf = vec![0.0f32; STEP * 2];
        while got.len() < frames {
            m.render(&mut buf);
            got.extend(buf.chunks(2).map(|f| f[0]));
            pace();
        }
        got.truncate(frames);
        got
    }

    /// Pull `frames` straight from a source at real-time pace, silence included.
    fn read_paced(src: &mut StreamingSource, frames: usize) -> Vec<f32> {
        let mut got = Vec::with_capacity(frames);
        let mut buf = vec![0.0f32; STEP];
        while got.len() < frames {
            let want = (frames - got.len()).min(STEP);
            let k = src.read(&mut buf[..want]);
            got.extend_from_slice(&buf[..k]);
            pace();
        }
        got
    }

    fn expected(n: usize, total: usize) -> Vec<f32> {
        signal(n)
            .iter()
            .cycle()
            .take(total)
            .map(|&s| s as f32 / 32768.0)
            .collect()
    }

    fn wait_until_stopped(m: &mut Mixer) {
        let mut buf = vec![0.0f32; 64];
        for _ in 0..500 {
            m.render(&mut buf);
            if m.mode(0) == PlayMode::Stop {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("channel never stopped");
    }

    #[test]
    fn playback_starts_with_the_prefilled_block_and_no_gap_then_ends() {
        let n = BLOCK_FRAMES * 3 + 123;
        let mut m = Mixer::new(RATE, 1);
        m.assign(0, Box::new(StreamingSource::spawn(source(n))));
        m.set_mode(0, PlayMode::PlayOnce);
        assert_eq!(collect(&mut m, n), expected(n, n));
        wait_until_stopped(&mut m);
    }

    #[test]
    fn replay_after_a_full_play_is_gapless() {
        let n = BLOCK_FRAMES * 2 + 300;
        let mut m = Mixer::new(RATE, 1);
        m.assign(0, Box::new(StreamingSource::spawn(source(n))));
        m.set_mode(0, PlayMode::PlayOnce);
        assert_eq!(collect(&mut m, n), expected(n, n));
        wait_until_stopped(&mut m);
        // Replay: starts from the first sample with no silence in front.
        m.set_mode(0, PlayMode::PlayOnce);
        assert_eq!(collect(&mut m, n), expected(n, n));
        wait_until_stopped(&mut m);
        // And a third time, to prove the epoch bookkeeping repeats.
        m.set_mode(0, PlayMode::PlayOnce);
        assert_eq!(collect(&mut m, n), expected(n, n));
    }

    #[test]
    fn explicit_rewind_mid_stream_restarts_gaplessly() {
        let n = BLOCK_FRAMES * 3 + 123;
        let mut src = StreamingSource::spawn(source(n));
        let first = BLOCK_FRAMES + 500;
        assert_eq!(read_paced(&mut src, first), expected(n, first));
        assert!(src.rewind());
        assert_eq!(read_paced(&mut src, n), expected(n, n));
    }

    #[test]
    fn looping_wraps_without_a_gap_when_the_end_marker_is_already_queued() {
        // A short file: the worker reaches the end before looping is switched on, so the wrap
        // goes through the end marker and the cached head.
        let n = 1000;
        let mut m = Mixer::new(RATE, 1);
        m.assign(0, Box::new(StreamingSource::spawn(source(n))));
        std::thread::sleep(Duration::from_millis(60));
        m.set_mode(0, PlayMode::Loop);
        assert_eq!(collect(&mut m, n * 6), expected(n, n * 6));
        assert_eq!(m.mode(0), PlayMode::Loop);
    }

    #[test]
    fn looping_wraps_without_a_gap_when_enabled_before_the_end() {
        let n = BLOCK_FRAMES * 2 + 37;
        let mut m = Mixer::new(RATE, 1);
        m.assign(0, Box::new(StreamingSource::spawn(source(n))));
        m.set_mode(0, PlayMode::Loop);
        assert_eq!(collect(&mut m, n * 3), expected(n, n * 3));
    }

    #[test]
    fn looping_enabled_late_on_a_long_stream_is_gapless() {
        // The worker has already queued the end marker (file shorter than the queue) when the
        // loop flag arrives mid-play.
        let n = BLOCK_FRAMES + 700;
        let mut m = Mixer::new(RATE, 1);
        m.assign(0, Box::new(StreamingSource::spawn(source(n))));
        m.set_mode(0, PlayMode::PlayOnce);
        let mut got = collect(&mut m, 2048);
        m.set_mode(0, PlayMode::Loop);
        got.extend(collect(&mut m, n * 3 - 2048));
        // (2048 is a whole number of render steps, so no frames are dropped between the parts.)
        assert_eq!(got, expected(n, n * 3));
    }

    #[test]
    fn rewind_reports_failure_for_unrewindable_sources() {
        let (q, w) = crate::audio::mixer::QueueSource::new(1, RATE);
        w.push(&[0.25; 64]);
        let mut s = StreamingSource::spawn(Box::new(q));
        assert!(!s.can_rewind());
        assert!(!s.rewind());
    }

    /// Claims it can rewind but fails when asked.
    struct StuckSource {
        left: usize,
    }
    impl AudioSource for StuckSource {
        fn channels(&self) -> u16 {
            1
        }
        fn sample_rate(&self) -> u32 {
            RATE
        }
        fn read(&mut self, out: &mut [f32]) -> usize {
            let n = out.len().min(self.left);
            out[..n].fill(0.5);
            self.left -= n;
            n
        }
        fn rewind(&mut self) -> bool {
            false
        }
    }

    #[test]
    fn worker_rewind_failure_is_reported_and_ends_the_stream() {
        let mut s = StreamingSource::spawn(Box::new(StuckSource { left: 6000 }));
        let mut buf = vec![0.0f32; 4096];
        assert_eq!(s.read(&mut buf), 4096);
        assert!(s.rewind());
        // The cached head replays (taking half a second of playback), by which time the worker
        // has found the rewind impossible; the stream then ends instead of looping or going
        // silent.
        std::thread::sleep(Duration::from_millis(100));
        let mut total = 0;
        for _ in 0..1000 {
            let k = s.read(&mut buf);
            if k == 0 {
                break;
            }
            assert!(!(k == 1 && buf[0] == 0.0), "silence instead of an end");
            total += k;
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(total, BLOCK_FRAMES);
        assert!(s.rewind_failed());
        // From now on the source reports that it cannot rewind.
        assert!(!s.can_rewind());
        assert!(!s.rewind());
    }

    #[test]
    fn loop_mode_ends_the_stream_when_the_worker_cannot_rewind() {
        let mut m = Mixer::new(RATE, 1);
        m.assign(
            0,
            Box::new(StreamingSource::spawn(Box::new(StuckSource { left: 6000 }))),
        );
        m.set_mode(0, PlayMode::Loop);
        let mut buf = vec![0.0f32; STEP * 2];
        let mut audible = 0usize;
        // Real-time pace; a replay-forever bug would still be Loop after 3 s of audio.
        for _ in 0..(3 * RATE as usize / STEP) {
            m.render(&mut buf);
            audible += buf.chunks(2).filter(|f| f[0] != 0.0).count();
            if m.mode(0) == PlayMode::Stop {
                break;
            }
            pace();
        }
        assert_eq!(
            m.mode(0),
            PlayMode::Stop,
            "loop must end when rewind is impossible"
        );
        // Depending on whether the worker saw the loop flag before it hit the end, the failure
        // is found at once (6000 frames play) or at the first wrap, in which case the cached head
        // plays one extra time. Either way the replay is not repeated: the next rewind is refused.
        assert!(
            audible == 6000 || audible == 6000 + BLOCK_FRAMES,
            "played {audible} frames"
        );
    }

    /// Thread-local allocation counter, to prove the consumer side never allocates.
    mod counting {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        thread_local! {
            static ALLOCS: Cell<usize> = const { Cell::new(0) };
        }

        pub struct Counting;

        // SAFETY: forwards every call to the system allocator unchanged; the counter is a
        // const-initialised thread-local `Cell`, which needs no allocation or destructor.
        unsafe impl GlobalAlloc for Counting {
            unsafe fn alloc(&self, l: Layout) -> *mut u8 {
                ALLOCS.with(|c| c.set(c.get() + 1));
                System.alloc(l)
            }
            unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
                System.dealloc(p, l)
            }
            unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
                ALLOCS.with(|c| c.set(c.get() + 1));
                System.realloc(p, l, n)
            }
        }

        pub fn count() -> usize {
            ALLOCS.with(|c| c.get())
        }
    }

    #[global_allocator]
    static ALLOC: counting::Counting = counting::Counting;

    /// First read blocks long enough to make the 500 ms prefill wait time out.
    struct SlowStart {
        first: bool,
        left: usize,
    }
    impl AudioSource for SlowStart {
        fn channels(&self) -> u16 {
            1
        }
        fn sample_rate(&self) -> u32 {
            RATE
        }
        fn read(&mut self, out: &mut [f32]) -> usize {
            if self.first {
                self.first = false;
                std::thread::sleep(Duration::from_millis(900));
            }
            let n = out.len().min(self.left);
            out[..n].fill(0.25);
            self.left -= n;
            n
        }
        fn rewind(&mut self) -> bool {
            self.left = 9000;
            true
        }
    }

    #[test]
    fn late_first_block_is_accepted_without_allocating_on_the_consumer() {
        let mut s = StreamingSource::spawn(Box::new(SlowStart {
            first: true,
            left: 9000,
        }));
        // The prefill timed out: nothing was accepted yet, so the first block (and its head
        // copy) arrives during ordinary reads, as it would on the audio callback.
        assert!(!s.head_set);
        let mut buf = vec![0.0f32; STEP];
        let mut got = 0usize;
        let mut allocs = 0usize;
        for _ in 0..400 {
            let before = counting::count();
            let k = s.read(&mut buf);
            allocs += counting::count() - before;
            if k > 1 || buf[0] != 0.0 {
                got += k;
                if got >= 9000 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, 9000, "all data arrives after the slow start");
        assert!(s.head_set);
        assert_eq!(allocs, 0, "the consumer allocated while accepting blocks");
        // The head copy works: a replay starts at once.
        assert!(s.rewind());
        let k = s.read(&mut buf);
        assert!(k > 1 && buf[..k].iter().all(|&v| v == 0.25));
    }

    struct FastSource;
    impl AudioSource for FastSource {
        fn channels(&self) -> u16 {
            1
        }
        fn sample_rate(&self) -> u32 {
            u32::MAX
        }
        fn read(&mut self, out: &mut [f32]) -> usize {
            out.fill(0.1);
            out.len()
        }
        fn rewind(&mut self) -> bool {
            true
        }
    }

    #[test]
    fn absurd_source_rates_are_clamped() {
        let s = StreamingSource::spawn(Box::new(FastSource));
        assert_eq!(s.sample_rate(), MAX_SAMPLE_RATE);
    }
}
