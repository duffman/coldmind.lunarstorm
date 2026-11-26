//! A generational index arena: `Arena<T>` hands out `Key`s whose slot is retired, not
//! reused, when its generation is exhausted.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub slot: u32,
    pub generation: u64,
}
struct Slot<T> {
    generation: u64,
    value: Option<T>,
}
/// Slots written by [`Arena::new`]; further slots are written by the insert that first
/// needs them.
const EAGER_SLOTS: usize = 64;
pub(crate) struct Arena<T> {
    /// Initialised slots. The allocation holds `capacity` slots from the start; only the
    /// initialised prefix has been written.
    slots: Vec<Slot<T>>,
    /// Released and not yet used slot indices among the initialised ones.
    free: Vec<u32>,
    /// Number of occupied slots.
    len: usize,
    /// Upper bound of `slots.len()`.
    capacity: usize,
}
impl<T> Arena<T> {
    pub fn new(capacity: usize) -> Result<Self, ()> {
        if capacity > u32::MAX as usize {
            return Err(());
        }
        let mut slots = Vec::new();
        let mut free = Vec::new();
        slots.try_reserve_exact(capacity).map_err(|_| ())?;
        free.try_reserve_exact(capacity).map_err(|_| ())?;
        // Fix: every slot and free-list entry was written here, so each VM paid the page
        // faults and resident memory of its full job, message and operation limits before
        // running anything. Decision: both allocations are still made (and fail) here at
        // full size, but only the first `EAGER_SLOTS` slots are written; `insert` writes
        // one more slot whenever the free list is empty. Untouched pages stay unmapped.
        let eager = capacity.min(EAGER_SLOTS);
        slots.resize_with(eager, || Slot {
            generation: 0,
            value: None,
        });
        free.extend((0..eager as u32).rev());
        Ok(Self {
            slots,
            free,
            len: 0,
            capacity,
        })
    }
    pub fn insert(&mut self, value: T) -> Result<Key, T> {
        loop {
            let index = match self.free.pop() {
                Some(index) => index,
                None if self.slots.len() < self.capacity => {
                    // Within the capacity reserved by `new`: no reallocation.
                    self.slots.push(Slot {
                        generation: 0,
                        value: None,
                    });
                    (self.slots.len() - 1) as u32
                }
                None => return Err(value),
            };
            let slot = &mut self.slots[index as usize];
            if let Some(generation) = slot.generation.checked_add(1) {
                slot.generation = generation;
                slot.value = Some(value);
                self.len += 1;
                return Ok(Key {
                    slot: index,
                    generation,
                });
            }
        }
    }
    pub fn get(&self, key: Key) -> Option<&T> {
        let slot = self.slots.get(key.slot as usize)?;
        (slot.generation == key.generation)
            .then_some(slot.value.as_ref())
            .flatten()
    }
    pub fn get_mut(&mut self, key: Key) -> Option<&mut T> {
        let slot = self.slots.get_mut(key.slot as usize)?;
        if slot.generation != key.generation {
            return None;
        }
        slot.value.as_mut()
    }
    pub fn remove(&mut self, key: Key) -> Option<T> {
        let slot = self.slots.get_mut(key.slot as usize)?;
        if slot.generation != key.generation {
            return None;
        }
        let value = slot.value.take()?;
        self.len -= 1;
        if slot.generation != u64::MAX {
            self.free.push(key.slot);
        }
        Some(value)
    }
    pub fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.value.as_ref().map(|_| Key {
                slot: index as u32,
                generation: slot.generation,
            })
        })
    }
    pub fn len(&self) -> usize {
        self.len
    }
    /// Number of slots the arena can hold.
    ///
    /// # Returns
    /// The limit given to [`Arena::new`], whether or not every slot was written yet.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}
#[cfg(test)]
#[path = "../tests/unit/arena.rs"]
mod tests;
