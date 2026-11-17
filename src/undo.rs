use std::path::PathBuf;

pub const DEPTH: usize = 20;

pub enum Step {
    MovePage { from: PathBuf, to: PathBuf, pictures: Vec<(String, String)> },
    MoveSection { from: PathBuf, to: PathBuf },
    RenamePage { from: PathBuf, to: PathBuf, old_title: String, new_title: String },
    RenameSection { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf, entry: PathBuf, section: bool },
    Create { path: PathBuf, section: bool, entry: Option<PathBuf> },
}

pub struct Record {
    pub step: Step,
    pub label: String,
}

#[derive(Default)]
pub struct UndoLog {
    undo: Vec<Record>,
    redo: Vec<Record>,
}

impl UndoLog {
    /// A user action: newest on top, oldest dropped beyond DEPTH, redo cleared.
    pub fn push(&mut self, r: Record) {
        self.undo.push(r);
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn take_undo(&mut self) -> Option<Record> {
        self.undo.pop()
    }

    pub fn take_redo(&mut self) -> Option<Record> {
        self.redo.pop()
    }

    /// After a successful undo: the record goes onto the redo list.
    pub fn undone(&mut self, r: Record) {
        self.redo.push(r);
    }

    /// After a successful redo: the record goes back onto undo, redo is kept.
    pub fn redone(&mut self, r: Record) {
        self.undo.push(r);
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
    }

    /// Puts a record taken with `take_undo` back unchanged (the step could not start).
    pub fn return_undo(&mut self, r: Record) {
        self.undo.push(r);
    }

    /// Puts a record taken with `take_redo` back unchanged (the step could not start).
    pub fn return_redo(&mut self, r: Record) {
        self.redo.push(r);
    }

    // Used by the Task 5 hover texts; remove the allow then.
    #[allow(dead_code)]
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|r| r.label.as_str())
    }

    // Used by the Task 5 hover texts; remove the allow then.
    #[allow(dead_code)]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|r| r.label.as_str())
    }
}

/// Swaps each pair (used to reverse a picture-name mapping).
pub fn invert(pictures: &[(String, String)]) -> Vec<(String, String)> {
    pictures.iter().map(|(a, b)| (b.clone(), a.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(label: &str) -> Record {
        Record {
            step: Step::MoveSection { from: PathBuf::from("a"), to: PathBuf::from("b") },
            label: label.to_string(),
        }
    }

    #[test]
    fn push_caps_at_depth() {
        let mut log = UndoLog::default();
        for i in 0..=DEPTH {
            log.push(rec(&format!("r{i}")));
        }
        let mut n = 0;
        let mut last = String::new();
        while let Some(r) = log.take_undo() {
            n += 1;
            last = r.label;
        }
        assert_eq!(n, DEPTH);
        assert_eq!(last, "r1");
    }

    #[test]
    fn push_clears_redo() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        let r = log.take_undo().unwrap();
        log.undone(r);
        assert_eq!(log.redo_label(), Some("A"));
        log.push(rec("B"));
        assert_eq!(log.redo_label(), None);
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        let r = log.take_undo().unwrap();
        assert_eq!(log.undo_label(), None);
        log.undone(r);
        let r = log.take_redo().unwrap();
        assert_eq!(log.redo_label(), None);
        log.redone(r);
        assert_eq!(log.undo_label(), Some("A"));
    }

    #[test]
    fn redone_keeps_rest_of_redo() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let b = log.take_undo().unwrap();
        log.undone(b);
        let a = log.take_undo().unwrap();
        log.undone(a);
        let a = log.take_redo().unwrap();
        log.redone(a);
        assert_eq!(log.redo_label(), Some("B"));
        assert_eq!(log.undo_label(), Some("A"));
    }

    #[test]
    fn drop_failed_keeps_rest() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let _ = log.take_undo();
        assert_eq!(log.undo_label(), Some("A"));
    }

    #[test]
    fn return_undo_restores_unchanged() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let b = log.take_undo().unwrap();
        let a = log.take_undo().unwrap();
        log.undone(a);
        log.return_undo(b);
        assert_eq!(log.undo_label(), Some("B"));
        assert_eq!(log.redo_label(), Some("A"));
    }

    #[test]
    fn return_redo_restores_unchanged() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let b = log.take_undo().unwrap();
        log.undone(b);
        let a = log.take_undo().unwrap();
        log.undone(a);
        let a = log.take_redo().unwrap();
        log.return_redo(a);
        assert_eq!(log.redo_label(), Some("A"));
        assert_eq!(log.undo_label(), None);
    }

    #[test]
    fn invert_swaps_pairs() {
        let p = vec![("a".to_string(), "b".to_string()), ("c".to_string(), "d".to_string())];
        assert_eq!(
            invert(&p),
            vec![("b".to_string(), "a".to_string()), ("d".to_string(), "c".to_string())]
        );
    }
}
