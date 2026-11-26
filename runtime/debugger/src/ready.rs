//! FIFO queue of ready items, addressed by a caller-owned node index.
//!
//! Fix: the queue wrote every node at construction (`resize_with`), so a VM with the default
//! limits touched about 400 KB before it ran anything. Decision: nodes live in pages of
//! [`PAGE`] entries that are allocated on the first push into them, and links are `u32`
//! with a sentinel instead of `Option<usize>`. The node ranges of the item kinds are
//! disjoint and far apart, which rules out one vector that grows to the highest index.
//! A page allocation aborts on exhaustion like every other small allocation of the owner
//! thread; the page table itself is still reserved fallibly in [`ReadyQueue::new`].

/// Nodes per lazily allocated page.
const PAGE: usize = 256;
/// Link value of "no node".
const NONE: u32 = u32::MAX;

/// One queue position.
struct Node<K> {
    /// The queued item, `None` while the node is not linked.
    value: Option<K>,
    /// Index of the node queued before this one, or [`NONE`].
    previous: u32,
    /// Index of the node queued after this one, or [`NONE`].
    next: u32,
}

/// Intrusive doubly linked FIFO over a fixed index space.
pub(crate) struct ReadyQueue<K> {
    /// Page table; a page exists once a node inside it was pushed.
    pages: Vec<Option<Box<[Node<K>]>>>,
    /// Number of addressable node indices.
    capacity: usize,
    /// Oldest queued node.
    head: Option<u32>,
    /// Newest queued node.
    tail: Option<u32>,
    /// Number of queued nodes.
    len: usize,
}
impl<K: Copy> ReadyQueue<K> {
    /// Creates an empty queue.
    ///
    /// # Arguments
    /// * `capacity` - Number of node indices, `0..capacity`.
    ///
    /// # Errors
    /// `capacity` exceeds the `u32` link range, or the page table cannot be allocated.
    pub fn new(capacity: usize) -> Result<Self, ()> {
        if capacity >= NONE as usize {
            return Err(());
        }
        let count = capacity.div_ceil(PAGE);
        let mut pages = Vec::new();
        pages.try_reserve_exact(count).map_err(|_| ())?;
        pages.resize_with(count, || None);
        Ok(Self {
            pages,
            capacity,
            head: None,
            tail: None,
            len: 0,
        })
    }
    /// The node at `index`, allocating its page on first use.
    ///
    /// # Arguments
    /// * `index` - Node index below the capacity.
    ///
    /// # Returns
    /// The node.
    fn node_mut(&mut self, index: usize) -> &mut Node<K> {
        assert!(index < self.capacity, "ready node within capacity");
        let page = self.pages[index / PAGE].get_or_insert_with(|| {
            (0..PAGE)
                .map(|_| Node {
                    value: None,
                    previous: NONE,
                    next: NONE,
                })
                .collect()
        });
        &mut page[index % PAGE]
    }
    /// The node at `index` if its page exists.
    ///
    /// # Arguments
    /// * `index` - Node index.
    ///
    /// # Returns
    /// The node, or `None` when nothing was ever pushed into its page.
    fn existing(&mut self, index: usize) -> Option<&mut Node<K>> {
        self.pages
            .get_mut(index / PAGE)?
            .as_mut()
            .map(|page| &mut page[index % PAGE])
    }
    /// Appends `value` at node `index`.
    ///
    /// # Arguments
    /// * `index` - The caller's node; must not be queued already.
    /// * `value` - The item to queue.
    pub fn push(&mut self, index: usize, value: K) {
        let tail = self.tail;
        let node = self.node_mut(index);
        assert!(node.value.is_none(), "one ready node per slot");
        *node = Node {
            value: Some(value),
            previous: tail.unwrap_or(NONE),
            next: NONE,
        };
        let index = index as u32;
        if let Some(tail) = tail {
            self.node_mut(tail as usize).next = index;
        } else {
            self.head = Some(index);
        }
        self.tail = Some(index);
        self.len += 1;
    }
    /// Unlinks node `index`.
    ///
    /// # Arguments
    /// * `index` - The caller's node.
    ///
    /// # Returns
    /// The queued item, or `None` when the node was not queued.
    pub fn remove(&mut self, index: usize) -> Option<K> {
        let node = self.existing(index)?;
        let value = node.value.take()?;
        let (previous, next) = (node.previous, node.next);
        node.previous = NONE;
        node.next = NONE;
        if previous == NONE {
            self.head = (next != NONE).then_some(next);
        } else {
            self.node_mut(previous as usize).next = next;
        }
        if next == NONE {
            self.tail = (previous != NONE).then_some(previous);
        } else {
            self.node_mut(next as usize).previous = previous;
        }
        self.len -= 1;
        Some(value)
    }
    /// Removes the oldest queued item.
    ///
    /// # Returns
    /// The item, or `None` when the queue is empty.
    pub fn pop(&mut self) -> Option<K> {
        self.remove(self.head? as usize)
    }
    /// Number of queued items.
    ///
    /// # Returns
    /// The count.
    pub fn len(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
#[path = "../tests/unit/ready.rs"]
mod tests;
