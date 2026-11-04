//! The .pub container: an OLE compound file holding a few named streams.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::{Error, Result};

pub const CONTENTS: &str = "/Contents";
pub const ESCHER: &str = "/Escher/EscherStm";
pub const ESCHER_DELAY: &str = "/Escher/EscherDelayStm";
pub const QUILL: &str = "/Quill/QuillSub/CONTENTS";

/// The streams of a .pub file that the reader needs, loaded into memory.
#[derive(Debug, Clone)]
pub struct PubFile {
    pub streams: BTreeMap<String, Vec<u8>>,
}

impl PubFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut cf = cfb::open(path.as_ref())?;
        let mut streams = BTreeMap::new();
        for name in [CONTENTS, ESCHER, ESCHER_DELAY, QUILL] {
            if cf.is_stream(name) {
                let mut buf = Vec::new();
                cf.open_stream(name)?.read_to_end(&mut buf)?;
                streams.insert(name.to_string(), buf);
            }
        }
        if !streams.contains_key(CONTENTS) {
            return Err(Error::MissingStream("Contents"));
        }
        Ok(Self { streams })
    }

    pub fn stream(&self, name: &str) -> Option<&[u8]> {
        self.streams.get(name).map(Vec::as_slice)
    }

    /// Escher streams present in the file (main and delay), by name.
    pub fn escher_streams(&self) -> impl Iterator<Item = (&str, &[u8])> {
        [ESCHER, ESCHER_DELAY]
            .into_iter()
            .filter_map(|n| self.stream(n).map(|d| (n, d)))
    }
}

/// Copy `src` to `dst`, then overwrite the given streams in the copy.
/// Each replacement must be exactly as long as the original stream, so the
/// container layout never changes.
pub fn write_patched_copy(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    replacements: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    std::fs::copy(src.as_ref(), dst.as_ref())?;
    // the copy inherits a read-only original's attribute; the copy must be writable
    let mut perms = std::fs::metadata(dst.as_ref())?.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(dst.as_ref(), perms)?;
    }
    let file = std::fs::OpenOptions::new().read(true).write(true).open(dst.as_ref())?;
    let mut cf = cfb::CompoundFile::open(file)?;
    for (name, data) in replacements {
        let mut s = cf.open_stream(name)?;
        if s.len() != data.len() as u64 {
            return Err(Error::Malformed("patched stream must keep its size"));
        }
        s.seek(SeekFrom::Start(0))?;
        s.write_all(data)?;
    }
    cf.flush()?;
    Ok(())
}
