//! Bounded output queues and delivery acknowledgements.

use std::rc::Rc;

use mc_protocol::packets::client::RemoveEntities;

use super::frame::{encode, movement_frame, rendered};
use super::{Delivery, Frame, MAX_BYTES, MAX_FRAMES, Observer, VisibilityError};

impl Observer {
    /// Calling this again retries the same frame without advancing its baseline.
    /// Reconciliation/unload may cancel an admission that has not been written;
    /// callers must not reconcile while a transport write is in progress.
    pub fn next_frame(&mut self) -> Result<Option<Rc<[u8]>>, VisibilityError> {
        if let Some(frame) = &self.active {
            return Ok(Some(Rc::clone(&frame.bytes)));
        }
        self.active = self.priority.pop_front().or_else(|| self.reliable.pop_front());
        while self.active.is_none() {
            let Some(id) = self.motion_waiting.pop_front() else {
                return Ok(None);
            };
            let Some(player) = self.latest.remove(&id) else {
                continue;
            };
            if self.removal_pending(id) {
                continue;
            }
            let Some(previous) = self.rendered.get(&id) else {
                continue;
            };
            if previous.life_generation != player.life_generation {
                self.queue_remove(id, false)?;
                self.active = self.priority.pop_front().or_else(|| self.reliable.pop_front());
                continue;
            }
            let frame = movement_frame(player.clone(), previous)?;
            if let Some(frame) = frame {
                self.reserve(frame.bytes.len(), frame.frames)?;
                self.active = Some(frame);
            } else {
                // A revision can change without changing a quantized wire value.
                self.rendered.insert(id, rendered(&player));
            }
        }
        Ok(self.active.as_ref().map(|frame| Rc::clone(&frame.bytes)))
    }

    /// Commit only after the entire returned buffer was written and flushed.
    pub fn frame_sent(&mut self) {
        let Some(frame) = self.active.take() else { return };
        self.queued_bytes -= frame.bytes.len();
        self.queued_frames -= frame.frames;
        self.sent_bytes += frame.bytes.len() as u64;
        self.forget_pending(&frame.delivery);
        match frame.delivery {
            Delivery::RosterAdd(entries) => {
                self.roster.extend(entries);
            }
            Delivery::RosterRemove(id) => {
                self.roster.remove(&id);
            }
            Delivery::Spawn(player) | Delivery::Move(player) => {
                self.rendered.insert(player.entity_id, rendered(&player));
            }
            Delivery::Remove(id) => {
                self.rendered.remove(&id);
                self.latest.remove(&id);
                if self.desired.contains(&id) && !self.spawn_waiting.contains(&id) {
                    self.spawn_waiting.push_back(id);
                }
            }
        }
    }

    pub(super) fn reserve(&mut self, bytes: usize, frames: usize) -> Result<(), VisibilityError> {
        if bytes > MAX_BYTES - self.queued_bytes || frames > MAX_FRAMES - self.queued_frames {
            return Err(VisibilityError::Overflow);
        }
        self.queued_bytes += bytes;
        self.queued_frames += frames;
        self.high_water_bytes = self.high_water_bytes.max(self.queued_bytes);
        Ok(())
    }

    pub(super) fn push(&mut self, frame: Frame) -> Result<(), VisibilityError> {
        self.reserve(frame.bytes.len(), frame.frames)?;
        let id = frame.delivery.id();
        match &frame.delivery {
            Delivery::RosterAdd(entries) => {
                self.pending_roster.extend(entries.iter().map(|(id, _)| *id));
            }
            Delivery::RosterRemove(_) => {
                self.pending_departures.insert(id);
            }
            Delivery::Spawn(_) => {
                self.pending_spawns.insert(id);
            }
            Delivery::Remove(_) => {
                self.pending_removals.insert(id);
            }
            Delivery::Move(_) => {}
        }
        if frame.unload_barrier {
            self.priority.push_back(frame);
        } else {
            self.reliable.push_back(frame);
        }
        Ok(())
    }

    pub(super) fn replace(&mut self, index: usize, frame: Frame) -> Result<(), VisibilityError> {
        let previous = &self.reliable[index];
        let bytes = self.queued_bytes - previous.bytes.len();
        let frames = self.queued_frames - previous.frames;
        if frame.bytes.len() > MAX_BYTES - bytes || frame.frames > MAX_FRAMES - frames {
            return Err(VisibilityError::Overflow);
        }
        self.queued_bytes = bytes + frame.bytes.len();
        self.queued_frames = frames + frame.frames;
        self.high_water_bytes = self.high_water_bytes.max(self.queued_bytes);
        self.reliable[index] = frame;
        Ok(())
    }

    pub(super) fn cancel_where(&mut self, predicate: impl Fn(&Frame) -> bool) {
        if self.active.as_ref().is_some_and(&predicate) {
            let frame = self.active.take().expect("active frame exists");
            self.queued_bytes -= frame.bytes.len();
            self.queued_frames -= frame.frames;
            self.forget_pending(&frame.delivery);
        }
        let mut index = 0;
        while index < self.reliable.len() {
            if predicate(&self.reliable[index]) {
                let frame = self.reliable.remove(index).expect("queued frame exists");
                self.queued_bytes -= frame.bytes.len();
                self.queued_frames -= frame.frames;
                self.forget_pending(&frame.delivery);
            } else {
                index += 1;
            }
        }
    }

    pub(super) fn forget_pending(&mut self, delivery: &Delivery) {
        let id = delivery.id();
        match delivery {
            Delivery::RosterAdd(entries) => {
                for (id, _) in entries {
                    self.pending_roster.remove(id);
                }
            }
            Delivery::RosterRemove(_) => {
                self.pending_departures.remove(&id);
            }
            Delivery::Spawn(_) => {
                self.pending_spawns.remove(&id);
            }
            Delivery::Remove(_) => {
                self.pending_removals.remove(&id);
            }
            Delivery::Move(_) => {}
        }
    }

    pub(super) fn queue_remove(&mut self, id: i32, barrier: bool) -> Result<(), VisibilityError> {
        self.latest.remove(&id);
        if self.removal_pending(id) {
            if barrier
                && let Some(index) = self.reliable.iter().position(
                    |frame| matches!(frame.delivery, Delivery::Remove(other) if other == id),
                )
            {
                let mut frame = self.reliable.remove(index).expect("queued removal exists");
                frame.unload_barrier = true;
                self.priority.push_back(frame);
            }
            return Ok(());
        }
        self.push(Frame {
            bytes: encode(&RemoveEntities { entity_ids: &[id] })?,
            frames: 1,
            delivery: Delivery::Remove(id),
            unload_barrier: barrier,
        })
    }
}
