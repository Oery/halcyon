//! Lazy reliable reconciliation. Only one encoded frame can be awaiting a write.
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::rc::Rc;

use mc_protocol::packets::client::{
    ObjectiveAction, ResetScore, SetDisplayObjective, SetObjective,
};

use super::{Board, MAX_SCORES, ScoreHolder, ScoreboardError, encode};

/// Comparisons per reconciliation call, including unchanged scores. Large boards yield
/// rather than monopolizing the executor while looking for a changed entry.
const SCAN_BUDGET: usize = 256;

#[derive(Default)]
struct DeliveredObjective {
    name: Rc<str>,
    properties: u64,
    revision: Option<u64>,
    scores: BTreeMap<ScoreHolder, (u64, Rc<str>)>,
}

enum Commit {
    Create(u64, u64, Rc<str>),
    Properties(u64, u64),
    Remove(u64),
    Slot(usize, Option<u64>),
    Score(u64, ScoreHolder, u64, Rc<str>),
    Reset(u64, ScoreHolder),
}

struct Frame {
    bytes: Rc<[u8]>,
    commit: Commit,
}

struct Scan {
    objective: u64,
    revision: u64,
    after: Option<ScoreHolder>,
    removing: bool,
}

#[derive(Default)]
pub struct ScoreboardObserver {
    known: BTreeMap<u64, DeliveredObjective>,
    slots: [Option<u64>; 3],
    active: Option<Frame>,
    scan: Option<Scan>,
    scan_after: Option<u64>,
    pending: bool,
}

impl ScoreboardObserver {
    pub fn pending_work(&self) -> bool {
        self.pending
    }

    /// Repeated calls return the same frame until `sent`. Callers end the
    /// registry borrow before writing, and never reconcile during that write.
    pub fn next_frame(
        &mut self,
        board: &Board,
        viewer: i32,
    ) -> Result<Option<Rc<[u8]>>, ScoreboardError> {
        if let Some(frame) = &self.active {
            return Ok(Some(frame.bytes.clone()));
        }
        self.pending = true;
        let slots = board.selected(viewer);
        let mut desired = slots;
        desired.sort_unstable();

        // Release old objectives first, so rapid slot switching never grows
        // delivered state beyond the three objectives a viewer can select.
        if let Some((&serial, _)) =
            self.known.iter().find(|(serial, _)| !desired.contains(&Some(**serial)))
        {
            let name = &self.known[&serial].name;
            let bytes = encode(&SetObjective { name, action: ObjectiveAction::Remove })?;
            return self.frame(bytes, Commit::Remove(serial));
        }

        // New definitions precede scores and slot assignment. Metadata changes
        // use mode 2, preserving the existing objective and its entries.
        for &serial in desired.iter().flatten() {
            let objective = &board.objectives[&serial];
            let Some(known) = self.known.get(&serial) else {
                return self.frame(
                    objective.create.clone(),
                    Commit::Create(serial, objective.properties_revision, objective.name.clone()),
                );
            };
            if known.properties != objective.properties_revision {
                return self.frame(
                    objective.update.clone(),
                    Commit::Properties(serial, objective.properties_revision),
                );
            }
        }

        if let Some(frame) = self.slot_frame(board, &slots)? {
            return Ok(Some(frame));
        }

        let mut budget = SCAN_BUDGET;
        loop {
            if self.scan.as_ref().is_some_and(|scan| !desired.contains(&Some(scan.objective))) {
                self.scan = None;
            }
            if self.scan.is_none() {
                let dirty = |serial: &u64| {
                    self.known[serial].revision != Some(board.objectives[serial].revision)
                };
                let next = desired
                    .iter()
                    .flatten()
                    .copied()
                    .find(|serial| {
                        self.scan_after.is_none_or(|after| *serial > after) && dirty(serial)
                    })
                    .or_else(|| desired.iter().flatten().copied().find(dirty));
                let Some(serial) = next else { break };
                self.scan = Some(Scan {
                    objective: serial,
                    revision: board.objectives[&serial].revision,
                    after: None,
                    removing: true,
                });
            }
            let scan = self.scan.as_mut().unwrap();
            let serial = scan.objective;
            let objective = &board.objectives[&serial];
            let delivered_scores: usize = self.known.values().map(|known| known.scores.len()).sum();
            let known = &mut self.known.get_mut(&serial).unwrap();
            let start = scan.after.as_ref().map_or(Unbounded, Excluded);
            if scan.removing {
                let next = known.scores.range::<ScoreHolder, _>((start, Unbounded)).next();
                if let Some((key, (_, holder))) = next {
                    if budget == 0 {
                        return Ok(None);
                    }
                    budget -= 1;
                    scan.after = Some(key.clone());
                    if objective.score(key).is_none() {
                        let name = &objective.name;
                        let bytes = encode(&ResetScore { holder, objective: Some(name) })?;
                        let key = key.clone();
                        return self.frame(bytes, Commit::Reset(serial, key));
                    }
                } else {
                    scan.removing = false;
                    scan.after = None;
                }
            } else {
                let next = objective.next_score(scan.after.as_ref());
                if let Some((key, entry)) = next {
                    if budget == 0 {
                        return Ok(None);
                    }
                    budget -= 1;
                    scan.after = Some(key.clone());
                    if known.scores.get(key).is_none_or(|(revision, _)| *revision != entry.revision)
                    {
                        // Scores can disappear behind the cursor while new
                        // keys arrive ahead of it. Bound delivered baselines as
                        // well as owner state under sustained membership churn.
                        // A fresh definition frees stale scores without keeping
                        // an unbounded history for this slow viewer.
                        if !known.scores.contains_key(key) && delivered_scores == MAX_SCORES {
                            let victim = self
                                .known
                                .iter()
                                .max_by_key(|(_, known)| known.scores.len())
                                .unwrap()
                                .0;
                            let victim = *victim;
                            let name = &self.known[&victim].name;
                            let bytes =
                                encode(&SetObjective { name, action: ObjectiveAction::Remove })?;
                            return self.frame(bytes, Commit::Remove(victim));
                        }
                        return self.frame(
                            entry.frame.clone(),
                            Commit::Score(
                                serial,
                                key.clone(),
                                entry.revision,
                                entry.holder.clone(),
                            ),
                        );
                    }
                } else {
                    // Edits during a pass require another pass. The cursor
                    // finishes this pass first, so low keys cannot starve others.
                    known.revision = Some(scan.revision);
                    self.scan_after = Some(serial);
                    self.scan = None;
                }
            }
        }

        if let Some(frame) = self.slot_frame(board, &slots)? {
            return Ok(Some(frame));
        }
        self.pending = false;
        Ok(None)
    }

    /// A constantly updated objective must not delay other slots or keep a
    /// hydrated display hidden while it catches up with newer revisions.
    fn slot_frame(
        &mut self,
        board: &Board,
        slots: &[Option<u64>; 3],
    ) -> Result<Option<Rc<[u8]>>, ScoreboardError> {
        for (slot, desired) in slots.iter().enumerate() {
            let ready = desired.is_none_or(|serial| self.known[&serial].revision.is_some());
            if self.slots[slot] != *desired && ready {
                let name = desired
                    .map(|serial| board.objectives[&serial].name.as_ref())
                    .unwrap_or_default();
                let bytes = encode(&SetDisplayObjective { slot: slot as i32, name })?;
                return self.frame(bytes, Commit::Slot(slot, *desired));
            }
        }
        Ok(None)
    }

    fn frame(
        &mut self,
        bytes: Rc<[u8]>,
        commit: Commit,
    ) -> Result<Option<Rc<[u8]>>, ScoreboardError> {
        self.active = Some(Frame { bytes: bytes.clone(), commit });
        Ok(Some(bytes))
    }

    pub fn sent(&mut self) {
        let Some(frame) = self.active.take() else { return };
        match frame.commit {
            Commit::Create(serial, properties, name) => {
                self.known
                    .insert(serial, DeliveredObjective { name, properties, ..Default::default() });
            }
            Commit::Properties(serial, revision) => {
                self.known.get_mut(&serial).unwrap().properties = revision;
            }
            Commit::Remove(serial) => {
                self.known.remove(&serial);
                if self.scan.as_ref().is_some_and(|scan| scan.objective == serial) {
                    self.scan = None;
                }
                for slot in &mut self.slots {
                    if *slot == Some(serial) {
                        *slot = None;
                    }
                }
            }
            Commit::Slot(slot, serial) => {
                self.slots[slot] = serial;
            }
            Commit::Score(serial, key, revision, holder) => {
                self.known.get_mut(&serial).unwrap().scores.insert(key, (revision, holder));
            }
            Commit::Reset(serial, key) => {
                self.known.get_mut(&serial).unwrap().scores.remove(&key);
            }
        }
    }

    /// At most three known objectives, even if a replacement was mid-delivery.
    /// Drop an unsent active frame; only successfully written definitions count.
    pub fn clear(&mut self) -> Result<Vec<Rc<[u8]>>, ScoreboardError> {
        let mut frames = Vec::new();
        for (slot, value) in self.slots.iter().enumerate() {
            if value.is_some() {
                frames.push(encode(&SetDisplayObjective { slot: slot as i32, name: "" })?);
            }
        }
        for objective in self.known.values() {
            let name = &objective.name;
            frames.push(encode(&SetObjective { name, action: ObjectiveAction::Remove })?);
        }
        *self = Self::default();
        Ok(frames)
    }

    /// Encoded bytes and holder strings remain shared with the owner when current.
    /// Their allocations are counted by `Board`, not once per observer.
    #[cfg(test)]
    pub(crate) fn retained_payload_bytes(&self) -> usize {
        use std::mem::size_of;
        size_of::<Self>()
            + self
                .known
                .values()
                .map(|objective| {
                    size_of::<(u64, DeliveredObjective)>()
                        + objective
                            .scores
                            .keys()
                            .map(|key| {
                                size_of::<(ScoreHolder, (u64, Rc<str>))>()
                                    + match key {
                                        ScoreHolder::Entry(key) => key.len(),
                                        _ => 0,
                                    }
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}
