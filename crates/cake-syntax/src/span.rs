//! Source spans for error reporting and syntax highlighting.

/// A byte offset into the source text.
pub type Offset = u32;

/// A half-open range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Offset,
    pub end: Offset,
}

impl Span {
    pub fn new(start: Offset, end: Offset) -> Self {
        Self { start, end }
    }

    /// An empty span at `pos`.
    pub fn at(pos: Offset) -> Self {
        Self {
            start: pos,
            end: pos,
        }
    }

    /// An empty span of unknown location.
    pub const UNKNOWN: Span = Span { start: 0, end: 0 };

    /// The smallest span covering both `self` and `other`.
    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn len(&self) -> usize {
        (self.end.saturating_sub(self.start)) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Whether `pos` lies within `[start, end)`.
    pub fn contains(&self, pos: Offset) -> bool {
        pos >= self.start && pos < self.end
    }
}
