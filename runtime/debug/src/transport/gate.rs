//! Pure sleep-gate transitions shared by native transport and concurrency models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Gate {
    Awake,
    Armed,
    Closed,
    Faulted,
}
impl Gate {
    /// Arm only after all authoritative queues were inspected under their lock.
    pub fn arm(&mut self, backlog: bool) -> bool {
        if backlog || matches!(self, Self::Closed | Self::Faulted) {
            return false;
        }
        *self = Self::Armed;
        true
    }
    pub fn awake(&mut self) {
        if *self == Self::Armed {
            *self = Self::Awake;
        }
    }
}
#[cfg(all(test, not(miri)))]
#[path = "../../tests/unit/gate.rs"]
mod tests;
