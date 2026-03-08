use crate::event::Event;
use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// A marker that records the start position of a syntax node in the event list.
///
/// Must be either completed via [`Marker::complete`] or abandoned via
/// [`Marker::abandon`]. Dropping a `Marker` without doing either will panic
/// (the "bomb" pattern).
pub struct Marker {
    pos: u32,
    bomb: DropBomb,
}

impl Marker {
    pub(crate) fn new(pos: u32) -> Marker {
        Marker {
            pos,
            bomb: DropBomb::new("Marker must be either completed or abandoned"),
        }
    }

    /// Finish the syntax node, assigning it the given [`SyntaxKind`].
    ///
    /// Returns a [`CompletedMarker`] that can be used to wrap this node in a
    /// parent via [`CompletedMarker::precede`].
    pub fn complete(mut self, p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
        self.bomb.defuse();
        let idx = self.pos as usize;
        match &mut p.events[idx] {
            Event::Start {
                kind: slot,
                forward_parent: _,
            } => {
                *slot = kind;
            }
            _ => unreachable!("expected Start event at marker position"),
        }
        p.push_event(Event::Finish);
        CompletedMarker { pos: self.pos }
    }

    /// Abandon this marker, converting its `Start` event into a `Tombstone`.
    ///
    /// This is used when tentative parsing decides this node should not exist.
    pub fn abandon(mut self, p: &mut Parser<'_>) {
        self.bomb.defuse();
        let idx = self.pos as usize;
        if idx == p.events.len() - 1 {
            match p.events.pop() {
                Some(Event::Start {
                    kind: SyntaxKind::TOMBSTONE,
                    forward_parent: None,
                }) => {}
                _ => unreachable!("expected uncommitted Start event"),
            }
        } else {
            // There are subsequent events; replace with Tombstone.
            p.events[idx] = Event::Tombstone;
        }
    }
}

/// Returned by [`Marker::complete`]. Allows wrapping a completed node in a
/// new parent node via [`CompletedMarker::precede`].
#[derive(Debug, Clone, Copy)]
pub struct CompletedMarker {
    pos: u32,
}

impl CompletedMarker {
    /// Create a new `Marker` that will become the parent of this completed node.
    ///
    /// This works by creating a new `Start` event at the current position and
    /// setting the `forward_parent` field of the original `Start` event to
    /// point to it. The tree builder resolves these forward-parent chains when
    /// constructing the CST.
    pub fn precede(self, p: &mut Parser<'_>) -> Marker {
        let new_pos = p.start_event();
        // Store as delta from child → parent (forward direction).
        let delta = new_pos - self.pos;
        match &mut p.events[self.pos as usize] {
            Event::Start {
                forward_parent, ..
            } => {
                *forward_parent = Some(delta);
            }
            _ => unreachable!("expected Start event at completed marker position"),
        }
        Marker::new(new_pos)
    }
}

// ---------------------------------------------------------------------------
// DropBomb — panics if not defused before being dropped
// ---------------------------------------------------------------------------

struct DropBomb {
    msg: &'static str,
    defused: bool,
}

impl DropBomb {
    fn new(msg: &'static str) -> DropBomb {
        DropBomb {
            msg,
            defused: false,
        }
    }

    fn defuse(&mut self) {
        self.defused = true;
    }
}

impl Drop for DropBomb {
    fn drop(&mut self) {
        if !self.defused && !std::thread::panicking() {
            panic!("{}", self.msg);
        }
    }
}
