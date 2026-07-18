//! The scheduler's single privilege-neutral ready queue.

use alloc::collections::VecDeque;

use super::entity::EntityId;

/// Hard bound keeps interrupt-side queue operations allocation-free after
/// scheduler initialization.
pub const MAX_ENTITIES: usize = 256;

pub struct RunQueue {
    queue: VecDeque<EntityId>,
}

impl RunQueue {
    pub const fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    pub fn reserve(&mut self) -> Result<(), ()> {
        self.queue.try_reserve(MAX_ENTITIES).map_err(|_| ())
    }

    pub fn enqueue(&mut self, id: EntityId) -> Result<bool, ()> {
        if self.contains(id) {
            return Ok(false);
        }
        if self.queue.len() >= MAX_ENTITIES {
            return Err(());
        }
        self.queue.push_back(id);
        Ok(true)
    }

    pub fn remove(&mut self, id: EntityId) -> bool {
        let Some(index) = self.queue.iter().position(|candidate| *candidate == id) else {
            return false;
        };
        self.queue.remove(index);
        true
    }

    pub fn remove_at(&mut self, index: usize) -> Option<EntityId> {
        self.queue.remove(index)
    }

    pub fn contains(&self, id: EntityId) -> bool {
        self.queue.iter().any(|candidate| *candidate == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &EntityId> {
        self.queue.iter()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}
