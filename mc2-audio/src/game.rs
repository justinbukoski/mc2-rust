// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The game sound system: the radio message queue on top of [`SoundSystem`].
//!
//! Port of `code/gamesound.cpp` / `gamesound.h` (`GameSoundSystem`).
//!
//! **Queue rules** (all from the C++, including its quirks):
//! - at most [`MAX_QUEUED_MESSAGES`] (8) messages wait; priority 0 is the most urgent. A new message is
//!   inserted before the first queued message of lower urgency (a larger number); if none is, it goes after
//!   the last. The queue shifts to make room, so when 7 or 8 messages are queued the least urgent ones fall
//!   off the end (with 7 waiting, inserting in the middle already drops the last).
//! - a priority-0 message cancels the one playing and removes everything else queued from the same pilot.
//! - two messages that name the same voice packet in the same turn are duplicates: the older is removed.
//! - `check_message`: a pilot is not allowed to queue something less urgent than what he already has
//!   queued, and a non-top-priority type that is already queued (by anyone) is not queued again.
//! - when the current message ends the next one starts; messages from pilots who are no longer active are
//!   dropped, except death and ejection calls.
//! - while a message plays: static, then each fragment, one per update once the previous finished. Radio
//!   volume is halved while Betty speaks.
//!
//! Difference from the C++: the number of queued messages is the number of entries (the C++ keeps a counter
//! that drifts by one when a message is inserted into a full queue, which later dereferences a null
//! message). The callbacks into the pilot and the GUI (`setMessagePlaying`, `clearMessagePlaying`,
//! `playPilotVideo`, `endPilotVideo`) become [`RadioEvent`]s the caller drains with
//! [`GameSoundSystem::take_radio_events`].

use crate::ids::sfx;
use crate::radio::{Noise, PilotId, RadioData, RadioMessageType};
use crate::system::{Refused, SoundSystem, BETTY_CHANNEL, MAX_QUEUED_MESSAGES, RADIO_CHANNEL};
use mc2_platform::audio::{PlayMode, Sound};
use std::sync::Arc;

/// `MAX_FRAGMENTS`.
pub const MAX_FRAGMENTS: usize = 16;
/// `ALARM_TIME`: seconds the general-alarm buzzer repeats.
pub const ALARM_TIME: f32 = 5.0;

/// A callback the C++ made into the pilot or the GUI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioEvent {
    /// `pilot->setMessagePlaying()`: the pilot is speaking.
    PilotSpeaking(PilotId),
    /// `pilot->clearMessagePlaying()`.
    PilotSilent(PilotId),
    /// `ControlGui::playPilotVideo(pilot, movieCode)`.
    PilotVideoStart { pilot: PilotId, movie_code: u8 },
    /// `ControlGui::endPilotVideo()`.
    PilotVideoEnd,
}

pub struct GameSoundSystem {
    /// The base sound system (`SoundSystem`).
    pub sys: SoundSystem,
    current_message: Option<Box<RadioData>>,
    queue: [Option<Box<RadioData>>; MAX_QUEUED_MESSAGES],
    current_fragment: usize,
    playing_noise: bool,
    whole_msg_done: bool,
    radio_handle: bool,
    general_alarm_timer: f32,
    /// The `GeneralAlarm` global: the perimeter alarm is sounding.
    pub general_alarm: bool,
    events: Vec<RadioEvent>,
}

impl GameSoundSystem {
    pub fn new(sys: SoundSystem) -> Self {
        Self {
            sys,
            current_message: None,
            queue: Default::default(),
            current_fragment: 0,
            playing_noise: false,
            whole_msg_done: true,
            radio_handle: false,
            general_alarm_timer: 0.0,
            general_alarm: false,
            events: Vec::new(),
        }
    }

    /// Pilot and GUI callbacks since the last call.
    pub fn take_radio_events(&mut self) -> Vec<RadioEvent> {
        std::mem::take(&mut self.events)
    }

    /// Messages waiting (not counting the one playing).
    pub fn messages_in_queue(&self) -> usize {
        self.queue.iter().filter(|q| q.is_some()).count()
    }

    pub fn current_message(&self) -> Option<&RadioData> {
        self.current_message.as_deref()
    }

    /// The waiting messages in play order.
    pub fn queued_messages(&self) -> impl Iterator<Item = &RadioData> {
        self.queue.iter().flatten().map(|b| b.as_ref())
    }

    /// `purgeSoundSystem`: end of mission. Drops the radio queue and the message playing and stops all
    /// sample channels except Betty's.
    pub fn purge_sound_system(&mut self) {
        self.whole_msg_done = true;
        self.current_message = None;
        self.current_fragment = 0;
        self.playing_noise = false;
        self.radio_handle = false;
        self.queue = Default::default();
        self.general_alarm_timer = 0.0;
        self.events.clear();
        self.sys.purge_channels();
    }

    fn clear_pilot(&mut self, pilot: PilotId) {
        self.events.push(RadioEvent::PilotSilent(pilot));
    }

    /// `removeQueuedMessage`.
    pub fn remove_queued_message(&mut self, index: usize) {
        if index >= MAX_QUEUED_MESSAGES {
            return;
        }
        if let Some(q) = self.queue[index].take() {
            self.clear_pilot(q.pilot);
            for i in index..MAX_QUEUED_MESSAGES - 1 {
                self.queue[i] = self.queue[i + 1].take();
            }
        }
    }

    /// `checkMessage`: may `pilot` queue a message of this priority and type?
    pub fn check_message(
        &self,
        pilot: PilotId,
        priority: u8,
        message_type: RadioMessageType,
    ) -> bool {
        for q in self.queue.iter().flatten() {
            if (q.pilot == pilot && priority > q.priority)
                || (q.priority > 1 && q.msg_type == message_type)
            {
                return false;
            }
        }
        true
    }

    /// `removeCurrentMessage`: drop the message playing and stop the radio channel.
    pub fn remove_current_message(&mut self) {
        if let Some(cur) = self.current_message.take() {
            self.clear_pilot(cur.pilot);
        }
        self.sys
            .mixer
            .lock()
            .set_mode(RADIO_CHANNEL, PlayMode::Stop);
        self.whole_msg_done = true;
    }

    /// `moveFromQueueToPlaying`.
    fn move_from_queue_to_playing(&mut self, pilot_active: &dyn Fn(PilotId) -> bool) {
        self.remove_current_message();
        while let Some(q) = &self.queue[0] {
            if !pilot_active(q.pilot)
                && q.msg_type != RadioMessageType::Death
                && q.msg_type != RadioMessageType::Ejecting
            {
                self.remove_queued_message(0);
            } else {
                break;
            }
        }
        self.current_message = self.queue[0].take();
        for i in 0..MAX_QUEUED_MESSAGES - 1 {
            self.queue[i] = self.queue[i + 1].take();
        }
    }

    /// `queueRadioMessage`. `Err` when the message was not queued (the queue is full of more urgent
    /// messages).
    pub fn queue_radio_message(&mut self, msg: Box<RadioData>) -> Result<(), Refused> {
        // Was this message already sent this turn? (`msgId >= MSG_TOTAL_MSGS` is always true.)
        for i in (0..MAX_QUEUED_MESSAGES).rev() {
            if let Some(q) = &self.queue[i] {
                if msg.turn_queued == q.turn_queued && msg.msg_id == q.msg_id {
                    self.remove_queued_message(i);
                }
            }
        }

        // Top priority cancels the message playing; it also means the pilot is gone, so his other
        // messages go too.
        if msg.priority == 0 {
            self.remove_current_message();
            for i in (0..MAX_QUEUED_MESSAGES).rev() {
                if self.queue[i].as_ref().is_some_and(|q| q.pilot == msg.pilot) {
                    self.remove_queued_message(i);
                }
            }
        }

        let mut i = 0;
        while i < MAX_QUEUED_MESSAGES {
            match &self.queue[i] {
                None => break,
                Some(q) if q.priority > msg.priority => {
                    // A more urgent message: push the rest down to make room.
                    for j in (i + 1..MAX_QUEUED_MESSAGES).rev() {
                        self.queue[j] = self.queue[j - 1].take();
                    }
                    // The old entry at `i` was moved to `i + 1`; the C++ copy leaves a duplicate at `i`
                    // that is overwritten below.
                    break;
                }
                Some(_) => i += 1,
            }
        }
        if i == MAX_QUEUED_MESSAGES {
            return Err(Refused::QueueFull);
        }
        if self.queue[MAX_QUEUED_MESSAGES - 1].is_some() {
            self.remove_queued_message(MAX_QUEUED_MESSAGES - 1);
        }
        self.queue[i] = Some(msg);
        Ok(())
    }

    // ---------------------------------------------------------------- playback

    /// Start a radio fragment: radio volume (halved while Betty speaks), assign, play once.
    fn start_radio_sound(&mut self, sound: &Arc<Sound>) {
        let betty = self.sys.is_channel_playing(BETTY_CHANNEL);
        let mut v = self.sys.digital_master_gain() * self.sys.radio_gain();
        if betty {
            v *= 0.5;
        }
        let mut m = self.sys.mixer.lock();
        m.set_volume(RADIO_CHANNEL, v);
        m.assign_sound(RADIO_CHANNEL, sound);
        m.set_mode(RADIO_CHANNEL, PlayMode::PlayOnce);
        self.radio_handle = true;
    }

    fn destroy_radio_handle(&mut self) {
        if self.radio_handle {
            self.sys.mixer.lock().release(RADIO_CHANNEL);
            self.radio_handle = false;
        }
    }

    /// The static before a message: play it if it loaded, and mark the pilot as speaking either way.
    fn start_noise(&mut self, noise: Noise, pilot: PilotId) {
        if let Noise::Sound(s) = noise {
            self.start_radio_sound(&s);
        }
        self.events.push(RadioEvent::PilotSpeaking(pilot));
        self.playing_noise = true;
    }

    /// `GameSoundSystem::update` with every pilot active.
    pub fn update(&mut self, frame_length: f32) {
        self.update_with(frame_length, &|_| true);
    }

    /// `GameSoundSystem::update`. `pilot_active` is `pilot->active()` for queued messages.
    pub fn update_with(&mut self, frame_length: f32, pilot_active: &dyn Fn(PilotId) -> bool) {
        self.sys.update(frame_length);

        if self.general_alarm && self.general_alarm_timer < ALARM_TIME {
            self.sys.play_digital_sample(sfx::BUZZER1, None, false);
            self.general_alarm_timer += frame_length;
        }

        let radio_mode = self.sys.mixer.lock().mode(RADIO_CHANNEL);
        if self.sys.use_sound && self.current_message.is_some() && radio_mode != PlayMode::PlayOnce
        {
            // Toss the radio data that just completed.
            self.destroy_radio_handle();

            if !self.whole_msg_done {
                self.advance_fragment();
            } else if let Some(cur) = &self.current_message {
                let pilot = cur.pilot;
                self.clear_pilot(pilot);
                self.events.push(RadioEvent::PilotVideoEnd);
                self.remove_current_message();
            }
        }

        if self.sys.use_sound && self.messages_in_queue() > 0 && self.whole_msg_done {
            self.destroy_radio_handle();

            // Done with the current sample: start the next.
            self.current_fragment = 0;
            self.move_from_queue_to_playing(pilot_active);

            // All remaining messages can go away because their pilots died.
            let Some(cur) = self.current_message.as_ref() else {
                return;
            };
            let pilot = cur.pilot;
            let movie_code = cur.movie_code;
            let noise = cur.noise[0].clone();
            let first = cur.data[0].clone();

            if movie_code != 0 {
                self.events
                    .push(RadioEvent::PilotVideoStart { pilot, movie_code });
            }

            if let Some(noise) = noise {
                self.start_noise(noise, pilot);
            } else {
                self.playing_noise = false;
                if let Some(s) = first {
                    self.start_radio_sound(&s);
                    self.events.push(RadioEvent::PilotSpeaking(pilot));
                }
            }
            self.whole_msg_done = false;
        }
    }

    /// The "finished a fragment, move on" branch of `update`.
    fn advance_fragment(&mut self) {
        let Some(cur) = self.current_message.as_ref() else {
            return;
        };
        let pilot = cur.pilot;
        let cf = self.current_fragment;
        if self.playing_noise {
            // The static is over: the fragment follows.
            self.playing_noise = false;
            if let Some(s) = cur.data.get(cf).cloned().flatten() {
                self.start_radio_sound(&s);
                self.events.push(RadioEvent::PilotSpeaking(pilot));
            } else {
                self.whole_msg_done = true;
            }
            self.current_fragment += 1;
        } else if let Some(noise) = cur.noise.get(cf).cloned().flatten() {
            self.start_noise(noise, pilot);
        } else {
            self.playing_noise = false;
            if let Some(s) = cur.data.get(cf).cloned().flatten() {
                self.start_radio_sound(&s);
                self.events.push(RadioEvent::PilotSpeaking(pilot));
            } else {
                self.whole_msg_done = true;
            }
            self.current_fragment += 1;
        }
    }
}
