//! The client's outgoing priority queue, the same for every client. Sockets and timers stay on
//! the platform side and call into this.
//!
//! | Level | What | Order |
//! |-------|------|-------|
//! | 0 | MLS commits, key changes | First |
//! | 1 | Is typing, stop typing | Next |
//! | 2 | Content of any kind, edit, delete | In the order sent |
//!
//! **Memory only**, so a client must show a message as pending until its sequence id arrives.
//! The queue knows nothing about what it carries, and the encrypted reply or reaction is built
//! only once [`Queue::take_next`] hands back its target's sequence id.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Keys,
    Typing,
    Content,
}

/// What an action points at. A pointer holds a sequence id and never a ULID, so an action whose
/// target is still pending has to wait for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    None,
    Seq(u64),
    Pending(u64),
}

/// Handed to the platform to send.
#[derive(Debug, PartialEq, Eq)]
pub struct Outgoing<T> {
    pub id: u64,
    pub item: T,
    pub pointer_seq: Option<u64>,
}

#[derive(Debug)]
struct Entry<T> {
    id: u64,
    level: Level,
    item: T,
    target: Target,
}

#[derive(Debug)]
pub struct Queue<T> {
    next_id: u64,
    keys: VecDeque<Entry<T>>,
    typing: VecDeque<Entry<T>>,
    content: VecDeque<Entry<T>>,
    /// Handed out and awaiting the server's answer, oldest first.
    in_flight: Vec<Entry<T>>,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            next_id: 0,
            keys: VecDeque::new(),
            typing: VecDeque::new(),
            content: VecDeque::new(),
            in_flight: Vec::new(),
        }
    }
}

impl<T: Clone> Queue<T> {
    /// Add something to send, and get the id later calls refer to it by.
    pub fn push(&mut self, level: Level, item: T, target: Target) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let entry = Entry {
            id,
            level,
            item,
            target,
        };
        match level {
            Level::Keys => self.keys.push_back(entry),
            Level::Typing => self.typing.push_back(entry),
            Level::Content => self.content.push_back(entry),
        }
        id
    }

    /// The next thing to send, if anything may go. An action waiting for its target's sequence
    /// id holds back the whole content level, so that nothing can overtake it. With `only_typing`,
    /// as while a commit waits for its save or its answer, only typing may go, so nothing is sealed
    /// under a key that is about to change.
    pub fn take_next(&mut self, only_typing: bool) -> Option<Outgoing<T>> {
        let mut entry = None;
        if !only_typing {
            entry = self.keys.pop_front();
        }
        if entry.is_none() {
            entry = self.typing.pop_front();
        }
        if entry.is_none()
            && !only_typing
            && let Some(first) = self.content.front()
            && !matches!(first.target, Target::Pending(_))
        {
            entry = self.content.pop_front();
        }
        let entry = entry?;
        let pointer_seq = match entry.target {
            Target::Seq(seq) => Some(seq),
            Target::None | Target::Pending(_) => None,
        };
        let outgoing = Outgoing {
            id: entry.id,
            item: entry.item.clone(),
            pointer_seq,
        };
        // Typing is never saved or answered, so there is nothing to wait for.
        if entry.level != Level::Typing {
            self.in_flight.push(entry);
        }
        Some(outgoing)
    }

    /// The server gave `id` this sequence id, whether in its reply or found in the updates after
    /// a reconnect. Anything waiting on it can now go.
    pub fn confirm(&mut self, id: u64, seq: u64) {
        let mut index = 0;
        while index < self.in_flight.len() {
            if self.in_flight[index].id == id {
                self.in_flight.remove(index);
            } else {
                index += 1;
            }
        }
        for entry in self.content.iter_mut() {
            if entry.target == Target::Pending(id) {
                entry.target = Target::Seq(seq);
            }
        }
    }

    /// The connection dropped. Sent items not found in the updates go back to the front of their
    /// level, in their original order, to go again. Typing still queued is stale, so it goes.
    pub fn reconnected(&mut self) {
        self.typing.clear();
        while let Some(entry) = self.in_flight.pop() {
            match entry.level {
                Level::Keys => self.keys.push_front(entry),
                Level::Typing => {}
                Level::Content => self.content.push_front(entry),
            }
        }
    }

    /// Something handed out cannot go yet, such as a message whose chat is still being joined. It
    /// goes back to the front of its level, so nothing behind it overtakes it.
    pub fn put_back(&mut self, id: u64) {
        let mut index = 0;
        while index < self.in_flight.len() {
            if self.in_flight[index].id == id {
                let entry = self.in_flight.remove(index);
                if entry.level == Level::Keys {
                    self.keys.push_front(entry);
                } else {
                    self.content.push_front(entry);
                }
                return;
            }
            index += 1;
        }
    }

    /// What is still waiting to be handed out, in the order it would go.
    pub fn queued_list(&self) -> Vec<(u64, &T)> {
        let mut list = Vec::new();
        for entry in self.keys.iter().chain(&self.typing).chain(&self.content) {
            list.push((entry.id, &entry.item));
        }
        list
    }

    /// What was sent and not answered, oldest first, for matching against the updates by client
    /// send time and contents.
    pub fn in_flight_list(&self) -> Vec<(u64, &T)> {
        let mut list = Vec::with_capacity(self.in_flight.len());
        for entry in &self.in_flight {
            list.push((entry.id, &entry.item));
        }
        list
    }

    /// Give up on `id`, because it was refused, or a commit lost its race. Whatever was waiting
    /// on it can never get a sequence id, so it goes too. Returns every id that went.
    pub fn drop_with_dependents(&mut self, id: u64) -> Vec<u64> {
        let mut dropped = vec![id];
        self.keys.retain(|entry| entry.id != id);
        self.typing.retain(|entry| entry.id != id);
        self.in_flight.retain(|entry| entry.id != id);
        self.content.retain(|entry| entry.id != id);
        // A dependent can have its own dependents, and targets always come first in the queue.
        let mut remaining = VecDeque::with_capacity(self.content.len());
        while let Some(entry) = self.content.pop_front() {
            let mut waits_on_dropped = false;
            if let Target::Pending(target) = entry.target {
                waits_on_dropped = dropped.contains(&target);
            }
            if waits_on_dropped {
                dropped.push(entry.id);
            } else {
                remaining.push_back(entry);
            }
        }
        self.content = remaining;
        dropped
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
            && self.typing.is_empty()
            && self.content.is_empty()
            && self.in_flight.is_empty()
    }
}
