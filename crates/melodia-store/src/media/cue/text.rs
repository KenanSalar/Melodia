//! A sheet's bytes as text. Rippers wrote sheets in their Windows codepage, so one with no BOM that
//! isn't UTF-8 is guessed from the whole sheet at once: pooled, every title adds to one sample, and
//! a short CJK title alone guesses poorly.

use std::io::Read;
use std::path::Path;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE};

use melodia_core::error::AppError;

/// The largest sheet read. Ninety-nine tracks with every field a sheet holds fit many times over,
/// so a larger `.cue` is something else, and it is never read whole.
const MAX_SHEET_BYTES: u64 = 256 * 1024;

/// Reads and decodes the sheet at `path`.
pub(super) fn read_sheet(path: &Path) -> Result<String, AppError> {
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
    Ok(decode(bytes))
}

fn decode(bytes: Vec<u8>) -> String {
    if let Some((encoding, bom_length)) = Encoding::for_bom(&bytes) {
        return decoded(encoding, &bytes[bom_length..]);
    }
    if let Some(encoding) = bomless_utf16(&bytes) {
        return decoded(encoding, &bytes);
    }
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return text,
        Err(not_utf8) => not_utf8.into_bytes(),
    };
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(&bytes, true);
    decoded(detector.guess(None, Utf8Detection::Deny), &bytes)
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
