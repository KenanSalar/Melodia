//! A sheet's bytes as text, and lines of text back as its bytes. Rippers wrote sheets in their
//! Windows codepage, so one with no BOM that isn't UTF-8 is guessed from the whole sheet at once:
//! pooled, every title adds to one sample, and a short CJK title alone guesses poorly.

use std::io::Read;
use std::path::Path;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

use melodia_core::error::AppError;

/// The largest sheet read. Ninety-nine tracks with every field a sheet holds fit many times over,
/// so a larger `.cue` is something else, and it is never read whole.
const MAX_SHEET_BYTES: u64 = 256 * 1024;

/// Reads and decodes the sheet at `path`.
pub(super) fn read_sheet(path: &Path) -> Result<String, AppError> {
    let sheet = EncodedSheet::read(path)?;
    Ok(decoded(sheet.encoding, sheet.body()))
}

/// A sheet's bytes as read, and the encoding they decode in.
pub(super) struct EncodedSheet {
    bytes: Vec<u8>,
    encoding: &'static Encoding,
    /// The BOM's length, 0 where there is none.
    bom: usize,
}

/// One line as the file holds it.
pub(super) struct RawLine<'a> {
    /// The line's bytes, its ending included.
    bytes: &'a [u8],
    /// The line's text, its ending left off.
    pub(super) text: String,
    pub(super) ending: LineEnding,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LineEnding {
    /// The last line, where the file ends without one.
    None,
    Lf,
    CrLf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

/// A line of the sheet being written: one the file holds, kept as it is, or new text.
pub(super) enum OutLine<'a> {
    Kept(&'a RawLine<'a>),
    New(String, LineEnding),
}

impl EncodedSheet {
    pub(super) fn read(path: &Path) -> Result<Self, AppError> {
        let failed = |e| AppError::metadata(format!("Failed to read {}", path.display()), e);
        let file = std::fs::File::open(path).map_err(failed)?;
        let mut bytes = Vec::new();
        // One byte past the cap tells a sheet at the cap from a larger one.
        file.take(MAX_SHEET_BYTES + 1).read_to_end(&mut bytes).map_err(failed)?;
        if bytes.len() as u64 > MAX_SHEET_BYTES {
            return Err(AppError::metadata_msg(format!(
                "{} is larger than any cue sheet",
                path.display()
            )));
        }
        let (encoding, bom) = detect(&bytes);
        Ok(Self { bytes, encoding, bom })
    }

    /// A sheet held as text rather than in a file of its own, as a FLAC's `CUESHEET` comment is.
    pub(super) fn from_text(text: &str) -> Self {
        let bom = if text.starts_with('\u{FEFF}') { '\u{FEFF}'.len_utf8() } else { 0 };
        Self { bytes: text.as_bytes().to_vec(), encoding: UTF_8, bom }
    }

    fn body(&self) -> &[u8] {
        &self.bytes[self.bom..]
    }

    /// The sheet's lines in file order. They are split before they are decoded, so each keeps its
    /// own bytes: no trail byte of a multi-byte encoding the guess can pick is an LF.
    pub(super) fn lines(&self) -> Vec<RawLine<'_>> {
        let body = self.body();
        let (_, lf) = self.ending_units();
        let mut lines = Vec::new();
        let mut start = 0;
        for (index, unit) in body.chunks_exact(lf.len()).enumerate() {
            if unit == lf {
                let end = (index + 1) * lf.len();
                lines.push(self.line(&body[start..end]));
                start = end;
            }
        }
        if start < body.len() {
            lines.push(self.line(&body[start..]));
        }
        lines
    }

    fn line<'a>(&self, bytes: &'a [u8]) -> RawLine<'a> {
        let (cr, lf) = self.ending_units();
        let (content, ending) = match bytes.strip_suffix(lf) {
            None => (bytes, LineEnding::None),
            Some(line) => match line.strip_suffix(cr) {
                Some(content) => (content, LineEnding::CrLf),
                None => (line, LineEnding::Lf),
            },
        };
        RawLine { bytes, text: decoded(self.encoding, content), ending }
    }

    /// CR and LF as this sheet's code units.
    fn ending_units(&self) -> (&'static [u8], &'static [u8]) {
        if self.encoding == UTF_16LE {
            (b"\r\0", b"\n\0")
        } else if self.encoding == UTF_16BE {
            (b"\0\r", b"\0\n")
        } else {
            (b"\r", b"\n")
        }
    }

    /// `lines` in this sheet's encoding behind its BOM, a kept line as its own bytes. Where a new
    /// line holds a character the encoding can't, the whole sheet is written as UTF-8 behind a BOM
    /// naming it to the reader, and `origin` is logged.
    pub(super) fn encode(&self, lines: &[OutLine<'_>], origin: &Path) -> Vec<u8> {
        if let Some(bytes) = self.encode_in_place(lines) {
            return bytes;
        }
        log::info!(
            "Rewrote {} as UTF-8: {} can't hold the edit",
            origin.display(),
            self.encoding.name()
        );
        as_utf8_with_bom(lines)
    }

    fn encode_in_place(&self, lines: &[OutLine<'_>]) -> Option<Vec<u8>> {
        let mut bytes = self.bytes[..self.bom].to_vec();
        for line in lines {
            match line {
                OutLine::Kept(raw) => bytes.extend_from_slice(raw.bytes),
                OutLine::New(text, ending) => {
                    bytes.extend(self.encode_text(text)?);
                    bytes.extend(self.encode_text(ending.as_str())?);
                }
            }
        }
        Some(bytes)
    }

    /// `text` in this sheet's encoding, or `None` where a character doesn't fit. UTF-16 is spelled
    /// by hand, `encoding_rs` encoding it as UTF-8.
    fn encode_text(&self, text: &str) -> Option<Vec<u8>> {
        if self.encoding == UTF_16LE {
            return Some(text.encode_utf16().flat_map(u16::to_le_bytes).collect());
        }
        if self.encoding == UTF_16BE {
            return Some(text.encode_utf16().flat_map(u16::to_be_bytes).collect());
        }
        // An unmappable character comes back as a numeric character reference, never an error.
        let (bytes, _, unmappable) = self.encoding.encode(text);
        (!unmappable).then(|| bytes.into_owned())
    }
}

fn as_utf8_with_bom(lines: &[OutLine<'_>]) -> Vec<u8> {
    let mut text = String::from('\u{FEFF}');
    for line in lines {
        let (line_text, ending) = match line {
            OutLine::Kept(raw) => (raw.text.as_str(), raw.ending),
            OutLine::New(new, ending) => (new.as_str(), *ending),
        };
        text.push_str(line_text);
        text.push_str(ending.as_str());
    }
    text.into_bytes()
}

/// The encoding `bytes` decode in, and the length of the BOM that says so.
fn detect(bytes: &[u8]) -> (&'static Encoding, usize) {
    if let Some(found) = Encoding::for_bom(bytes) {
        return found;
    }
    if let Some(encoding) = bomless_utf16(bytes) {
        return (encoding, 0);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (UTF_8, 0);
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    (detector.guess(None, Utf8Detection::Deny), 0)
}

fn decoded(encoding: &'static Encoding, bytes: &[u8]) -> String {
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

/// UTF-16 with no BOM, told by its NULs. A sheet's keywords and times are ASCII, whose UTF-16 code
/// units carry a NUL in their high byte, so a quarter of the units holding one on the same side is
/// far past a stray NUL in single-byte text.
fn bomless_utf16(bytes: &[u8]) -> Option<&'static Encoding> {
    let units = bytes.len() / 2;
    let (mut nul_first, mut nul_second) = (0, 0);
    for unit in bytes.chunks_exact(2) {
        nul_first += usize::from(unit[0] == 0);
        nul_second += usize::from(unit[1] == 0);
    }
    if nul_second > nul_first && nul_second * 4 >= units {
        Some(UTF_16LE)
    } else if nul_first > nul_second && nul_first * 4 >= units {
        Some(UTF_16BE)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "tests/text_tests.rs"]
mod tests;
