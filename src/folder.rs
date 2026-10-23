use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "webp", "bmp", "tif", "tiff", "svg",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// The images in one folder, in the order Finder and Explorer show them.
pub struct Folder {
    files: Vec<PathBuf>,
    index: usize,
}

impl Folder {
    /// Lists the folder containing `path` and points at `path`.
    ///
    /// `path` is kept even when its extension isn't recognized: the user chose to open it.
    pub fn open(path: &Path) -> io::Result<Self> {
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && is_supported(p) && !is_hidden(p))
            .collect();
        if !files.iter().any(|p| p == path) {
            files.push(path.to_path_buf());
        }
        files.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
        let index = files.iter().position(|p| p == path).unwrap_or(0);
        Ok(Self { files, index })
    }

    pub fn current(&self) -> Option<&Path> {
        self.files.get(self.index).map(PathBuf::as_path)
    }

    /// 1-based position and total, for the title bar.
    pub fn position(&self) -> (usize, usize) {
        (self.index + 1, self.files.len())
    }

    /// Moves by `delta`, stopping at either end. Returns whether the index changed.
    pub fn step(&mut self, delta: isize) -> bool {
        let last = self.files.len().saturating_sub(1) as isize;
        let next = (self.index as isize + delta).clamp(0, last) as usize;
        let moved = next != self.index;
        self.index = next;
        moved
    }

    /// Drops the current file (it was deleted) and lands on its neighbour in `direction`.
    pub fn remove_current(&mut self, direction: isize) {
        if self.files.is_empty() {
            return;
        }
        self.files.remove(self.index);
        // After removal the next file already sits at `index`; going backwards needs one step back.
        if direction < 0 || self.index >= self.files.len() {
            self.index = self.index.saturating_sub(1);
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_hidden(path: &Path) -> bool {
    file_name(path).starts_with('.')
}

/// Case-insensitive comparison where digit runs compare by value, so `img2` sorts before `img10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a_chars, mut b_chars) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a_chars.peek().copied(), b_chars.peek().copied()) {
            (None, None) => return a.cmp(b), // equal ignoring case: keep the order deterministic
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take_digits = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    digits
                };
                let (da, db) = (take_digits(&mut a_chars), take_digits(&mut b_chars));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a_chars.next();
                b_chars.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder_with(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        dir
    }

    fn names(folder: &Folder) -> Vec<String> {
        folder.files.iter().map(|p| file_name(p)).collect()
    }

    #[test]
    fn numbers_sort_by_value_like_finder() {
        let mut v = vec!["img10.jpg", "IMG2.jpg", "img1.jpg", "img02b.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["img1.jpg", "IMG2.jpg", "img02b.jpg", "img10.jpg"]);
    }

    #[test]
    fn lists_only_visible_supported_images() {
        let dir = folder_with(&["b.PNG", "a.jpg", "notes.txt", ".hidden.jpg", "c.webp"]);
        let folder = Folder::open(&dir.path().join("b.PNG")).unwrap();
        assert_eq!(names(&folder), ["a.jpg", "b.PNG", "c.webp"]);
        assert_eq!(folder.position(), (2, 3));
    }

    #[test]
    fn keeps_the_opened_file_even_with_unknown_extension() {
        let dir = folder_with(&["a.jpg", "photo.dat"]);
        let folder = Folder::open(&dir.path().join("photo.dat")).unwrap();
        assert_eq!(folder.current().unwrap(), dir.path().join("photo.dat"));
    }

    #[test]
    fn stepping_stops_at_both_ends() {
        let dir = folder_with(&["1.jpg", "2.jpg"]);
        let mut folder = Folder::open(&dir.path().join("1.jpg")).unwrap();
        assert!(!folder.step(-1));
        assert!(folder.step(1));
        assert!(!folder.step(1));
        assert_eq!(folder.position(), (2, 2));
    }

    #[test]
    fn removing_a_deleted_file_continues_in_the_travel_direction() {
        let dir = folder_with(&["1.jpg", "2.jpg", "3.jpg"]);
        let mut forward = Folder::open(&dir.path().join("2.jpg")).unwrap();
        forward.remove_current(1);
        assert_eq!(forward.current().unwrap(), dir.path().join("3.jpg"));

        let mut backward = Folder::open(&dir.path().join("2.jpg")).unwrap();
        backward.remove_current(-1);
        assert_eq!(backward.current().unwrap(), dir.path().join("1.jpg"));

        let mut last = Folder::open(&dir.path().join("3.jpg")).unwrap();
        last.remove_current(1);
        assert_eq!(last.current().unwrap(), dir.path().join("2.jpg"));
    }
}
