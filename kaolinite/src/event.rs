/// event.rs - manages editing events and provides tools for error handling
use crate::{document::Cursor, utils::Loc, Document};
use quick_error::quick_error;
use ropey::Rope;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    content: Rope,
    cursor: Cursor,
}

/// Represents an editing event.
/// All possible editing events can be made up of a combination these events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Insert(Loc, String),
    Delete(Loc, String),
    InsertLine(usize, String),
    DeleteLine(usize, String),
    SplitDown(Loc),
    SpliceUp(Loc),
}

impl Event {
    /// Given an event, provide the opposite of that event (for purposes of undoing)
    #[must_use]
    pub fn reverse(self) -> Event {
        match self {
            Event::Insert(loc, ch) => Event::Delete(loc, ch),
            Event::Delete(loc, ch) => Event::Insert(loc, ch),
            Event::InsertLine(loc, st) => Event::DeleteLine(loc, st),
            Event::DeleteLine(loc, st) => Event::InsertLine(loc, st),
            Event::SplitDown(loc) => Event::SpliceUp(loc),
            Event::SpliceUp(loc) => Event::SplitDown(loc),
        }
    }

    /// Get the location of an event
    #[must_use]
    pub fn loc(self) -> Loc {
        match self {
            Event::Insert(loc, _) => loc,
            Event::Delete(loc, _) => loc,
            Event::InsertLine(loc, _) => Loc { x: 0, y: loc },
            Event::DeleteLine(loc, _) => Loc { x: 0, y: loc },
            Event::SplitDown(loc) => loc,
            Event::SpliceUp(loc) => loc,
        }
    }
}

/// Represents various statuses of functions
#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    StartOfFile,
    EndOfFile,
    StartOfLine,
    EndOfLine,
    None,
}

/// Easy result type for unified error handling
pub type Result<T> = std::result::Result<T, Error>;

quick_error! {
    /// Error enum for handling all possible errors
    #[derive(Debug)]
    pub enum Error {
        Io(err: std::io::Error) {
            from()
            display("I/O error: {}", err)
            source(err)
        }
        Rope(err: ropey::Error) {
            from()
            display("Rope error: {}", err)
            source(err)
        }
        NoFileName
        OutOfRange
        ReadOnlyFile
    }
}

/// For managing events for purposes of undo and redo
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct UndoMgmt {
    /// Whether the file touched since the latest commit
    pub is_dirty: bool,
    /// Undo contains all the patches that have been applied
    pub undo: Vec<Snapshot>,
    /// Redo contains all the patches that have been undone
    pub redo: Vec<Snapshot>,
    /// Store where the file on the disk is currently at
    pub on_disk: usize,
}

impl Document {
    pub fn take_snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.file.clone(),
            cursor: self.cursor,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.file = snapshot.content;
        self.cursor = snapshot.cursor;
        self.char_ptr = self.character_idx(&snapshot.cursor.loc);
        self.reload_lines();
        self.bring_cursor_in_viewport();
    }
}

impl UndoMgmt {
    /// Register that an event has occurred and the last snapshot is not update
    pub fn set_dirty(&mut self) {
        self.redo.clear();
        self.is_dirty = true;
    }

    /// This will commit take a snapshot and add it to the undo stack, ready to be undone.
    /// You can call this after every space character, for example, which would
    /// make it so that every undo action would remove the previous word the user typed.
    pub fn commit(&mut self, current_snapshot: Snapshot) {
        if self.is_dirty {
            self.is_dirty = false;
            self.undo.push(current_snapshot);
        }
    }

    /// Provide a snapshot of the desired state of the document for purposes
    /// of undoing
    pub fn undo(&mut self, current_snapshot: Snapshot) -> Option<Snapshot> {
        self.commit(current_snapshot);
        if self.undo.len() < 2 {
            return None;
        }
        let snapshot_to_remove = self.undo.pop()?;
        let snapshot_to_apply = self.undo.last()?.clone();
        self.redo.push(snapshot_to_remove);

        Some(snapshot_to_apply)
    }

    /// Provide a snapshot of the desired state of the document for purposes of
    /// redoing
    pub fn redo(&mut self) -> Option<Snapshot> {
        let ev = self.redo.pop()?;
        self.undo.push(ev.clone());
        Some(ev)
    }

    /// On file save, mark where the document is to match it on the disk
    pub fn saved(&mut self) {
        self.on_disk = self.undo.len()
    }

    /// Determine if the state of the document is currently that of what is on the disk
    pub fn at_file(&self) -> bool {
        self.undo.len() == self.on_disk
    }
}
