//! Port of ufo-extractor's tests/stream_test.py.
//!
//! The Python tests compile assembly samples with fontTools before
//! disassembling them; the compiled bytecode is stored in tests/data/stream/
//! (see tests/data/regenerate.py).

use read_fonts::{FontRef, TableProvider};
use ttf2ufo::instructions::{ttx_assembly, vtt_assembly};

const EXPECTED_VTT: &str = "#PUSHOFF
#PUSH, 1, 2, 3, 4
#PUSH, 1, -1
#PUSH, 5, 512
SPVTCA[Y]\t/* SetPVectorToAxis */
SPVTCA[X]\t/* SetPVectorToAxis */
SFVTL[r]\t/* SetFVectorToLine */
SFVTL[R]\t/* SetFVectorToLine */
MDAP[r]\t/* MoveDirectAbsPt */
MDAP[R]\t/* MoveDirectAbsPt */
IF[]\t/* If */
  SHP[2]\t/* ShiftPointByLastPoint */
  SHC[1]\t/* ShiftContourByLastPt */
EIF[]\t/* EndIf */
MSIRP[M]\t/* MoveStackIndirRelPt */
MD[N]\t/* MeasureDistance */
GC[O]\t/* GetCoordOnPVector */
ROUND[Bl]\t/* Round */
MDRP[m<RWh]\t/* MoveDirectRelPt */
#PUSHON";

fn data(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn read_text(name: &str) -> String {
    std::fs::read_to_string(data(name)).unwrap()
}

fn read_bytes(name: &str) -> Vec<u8> {
    std::fs::read(data(name)).unwrap()
}

/// Like the Python tests: every produced line must equal the expected line at
/// the same index.
fn assert_lines(actual: &str, expected: &str) {
    let expected: Vec<&str> = expected.lines().collect();
    for (i, line) in actual.lines().enumerate() {
        assert_eq!(Some(&line), expected.get(i), "line {}", i + 1);
    }
}

fn plex_fpgm() -> Vec<u8> {
    let font_data = read_bytes("fonts/IBM Plex Serif-Text-FL.ttf");
    let font = FontRef::new(&font_data).unwrap();
    font.fpgm().unwrap().to_vec()
}

#[test]
fn test_extract_ttx() {
    let expected = read_text("stream/IBM Plex Serif-Text-FL.fpgm.ttxasm");
    assert_lines(&ttx_assembly(&plex_fpgm()).unwrap(), &expected);
}

#[test]
fn test_extract_vtt() {
    let expected = read_text("stream/IBM Plex Serif-Text-FL.fpgm.vttasm");
    assert_lines(&vtt_assembly(&plex_fpgm()).unwrap(), &expected);
}

#[test]
fn test_complex_ttx() {
    // Roundtripped sample should be identical
    let sample = read_text("stream/sample.ttxasm");
    assert_lines(&ttx_assembly(&read_bytes("stream/sample.bin")).unwrap(), &sample);
}

#[test]
fn test_complex_vtt() {
    assert_lines(&vtt_assembly(&read_bytes("stream/sample.bin")).unwrap(), EXPECTED_VTT);
}

#[test]
fn test_illegal_ttx() {
    // Sample contains illegal flags, ttx does ignore this
    let sample = read_text("stream/sample_keyerror.ttxasm");
    assert_eq!(ttx_assembly(&read_bytes("stream/sample_keyerror.bin")).unwrap(), sample);
}

#[test]
fn test_illegal_vtt() {
    // Sample contains illegal flags, vtt should raise an error
    assert!(vtt_assembly(&read_bytes("stream/sample_keyerror.bin")).is_err());
}
