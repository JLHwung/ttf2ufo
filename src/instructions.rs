//! TrueType bytecode disassembly: a port of ufo-extractor's `InstructionStream`
//! (extractor/stream.py), producing fontTools ("ttx") or VTT assembly.

use crate::Error;

/// (opcode, mnemonic, argBits, descriptive name), as in fontTools' ttProgram.
const STREAM_INSTRUCTIONS: &[(u8, &str, u8, &str)] = &[
    (0x40, "NPUSHB", 0, "PushNBytes"),
    (0x41, "NPUSHW", 0, "PushNWords"),
    (0xb0, "PUSHB", 3, "PushBytes"),
    (0xb8, "PUSHW", 3, "PushWords"),
];

const INSTRUCTIONS: &[(u8, &str, u8, &str)] = &[
    (0x7f, "AA", 0, "AdjustAngle"),
    (0x64, "ABS", 0, "Absolute"),
    (0x60, "ADD", 0, "Add"),
    (0x27, "ALIGNPTS", 0, "AlignPts"),
    (0x3c, "ALIGNRP", 0, "AlignRelativePt"),
    (0x5a, "AND", 0, "LogicalAnd"),
    (0x2b, "CALL", 0, "CallFunction"),
    (0x67, "CEILING", 0, "Ceiling"),
    (0x25, "CINDEX", 0, "CopyXToTopStack"),
    (0x22, "CLEAR", 0, "ClearStack"),
    (0x4f, "DEBUG", 0, "DebugCall"),
    (0x73, "DELTAC1", 0, "DeltaExceptionC1"),
    (0x74, "DELTAC2", 0, "DeltaExceptionC2"),
    (0x75, "DELTAC3", 0, "DeltaExceptionC3"),
    (0x5d, "DELTAP1", 0, "DeltaExceptionP1"),
    (0x71, "DELTAP2", 0, "DeltaExceptionP2"),
    (0x72, "DELTAP3", 0, "DeltaExceptionP3"),
    (0x24, "DEPTH", 0, "GetDepthStack"),
    (0x62, "DIV", 0, "Divide"),
    (0x20, "DUP", 0, "DuplicateTopStack"),
    (0x59, "EIF", 0, "EndIf"),
    (0x1b, "ELSE", 0, "Else"),
    (0x2d, "ENDF", 0, "EndFunctionDefinition"),
    (0x54, "EQ", 0, "Equal"),
    (0x57, "EVEN", 0, "Even"),
    (0x2c, "FDEF", 0, "FunctionDefinition"),
    (0x4e, "FLIPOFF", 0, "SetAutoFlipOff"),
    (0x4d, "FLIPON", 0, "SetAutoFlipOn"),
    (0x80, "FLIPPT", 0, "FlipPoint"),
    (0x82, "FLIPRGOFF", 0, "FlipRangeOff"),
    (0x81, "FLIPRGON", 0, "FlipRangeOn"),
    (0x66, "FLOOR", 0, "Floor"),
    (0x46, "GC", 1, "GetCoordOnPVector"),
    (0x88, "GETINFO", 0, "GetInfo"),
    (0x91, "GETVARIATION", 0, "GetVariation"),
    (0x0d, "GFV", 0, "GetFVector"),
    (0x0c, "GPV", 0, "GetPVector"),
    (0x52, "GT", 0, "GreaterThan"),
    (0x53, "GTEQ", 0, "GreaterThanOrEqual"),
    (0x89, "IDEF", 0, "InstructionDefinition"),
    (0x58, "IF", 0, "If"),
    (0x8e, "INSTCTRL", 0, "SetInstrExecControl"),
    (0x39, "IP", 0, "InterpolatePts"),
    (0x0f, "ISECT", 0, "MovePtToIntersect"),
    (0x30, "IUP", 1, "InterpolateUntPts"),
    (0x1c, "JMPR", 0, "Jump"),
    (0x79, "JROF", 0, "JumpRelativeOnFalse"),
    (0x78, "JROT", 0, "JumpRelativeOnTrue"),
    (0x2a, "LOOPCALL", 0, "LoopAndCallFunction"),
    (0x50, "LT", 0, "LessThan"),
    (0x51, "LTEQ", 0, "LessThenOrEqual"),
    (0x8b, "MAX", 0, "Maximum"),
    (0x49, "MD", 1, "MeasureDistance"),
    (0x2e, "MDAP", 1, "MoveDirectAbsPt"),
    (0xc0, "MDRP", 5, "MoveDirectRelPt"),
    (0x3e, "MIAP", 1, "MoveIndirectAbsPt"),
    (0x8c, "MIN", 0, "Minimum"),
    (0x26, "MINDEX", 0, "MoveXToTopStack"),
    (0xe0, "MIRP", 5, "MoveIndirectRelPt"),
    (0x4b, "MPPEM", 0, "MeasurePixelPerEm"),
    (0x4c, "MPS", 0, "MeasurePointSize"),
    (0x3a, "MSIRP", 1, "MoveStackIndirRelPt"),
    (0x63, "MUL", 0, "Multiply"),
    (0x65, "NEG", 0, "Negate"),
    (0x55, "NEQ", 0, "NotEqual"),
    (0x5c, "NOT", 0, "LogicalNot"),
    (0x6c, "NROUND", 2, "NoRound"),
    (0x56, "ODD", 0, "Odd"),
    (0x5b, "OR", 0, "LogicalOr"),
    (0x21, "POP", 0, "PopTopStack"),
    (0x45, "RCVT", 0, "ReadCVT"),
    (0x7d, "RDTG", 0, "RoundDownToGrid"),
    (0x7a, "ROFF", 0, "RoundOff"),
    (0x8a, "ROLL", 0, "RollTopThreeStack"),
    (0x68, "ROUND", 2, "Round"),
    (0x43, "RS", 0, "ReadStore"),
    (0x3d, "RTDG", 0, "RoundToDoubleGrid"),
    (0x18, "RTG", 0, "RoundToGrid"),
    (0x19, "RTHG", 0, "RoundToHalfGrid"),
    (0x7c, "RUTG", 0, "RoundUpToGrid"),
    (0x77, "S45ROUND", 0, "SuperRound45Degrees"),
    (0x7e, "SANGW", 0, "SetAngleWeight"),
    (0x85, "SCANCTRL", 0, "ScanConversionControl"),
    (0x8d, "SCANTYPE", 0, "ScanType"),
    (0x48, "SCFS", 0, "SetCoordFromStackFP"),
    (0x1d, "SCVTCI", 0, "SetCVTCutIn"),
    (0x5e, "SDB", 0, "SetDeltaBaseInGState"),
    (0x86, "SDPVTL", 1, "SetDualPVectorToLine"),
    (0x5f, "SDS", 0, "SetDeltaShiftInGState"),
    (0x0b, "SFVFS", 0, "SetFVectorFromStack"),
    (0x04, "SFVTCA", 1, "SetFVectorToAxis"),
    (0x08, "SFVTL", 1, "SetFVectorToLine"),
    (0x0e, "SFVTPV", 0, "SetFVectorToPVector"),
    (0x34, "SHC", 1, "ShiftContourByLastPt"),
    (0x32, "SHP", 1, "ShiftPointByLastPoint"),
    (0x38, "SHPIX", 0, "ShiftZoneByPixel"),
    (0x36, "SHZ", 1, "ShiftZoneByLastPoint"),
    (0x17, "SLOOP", 0, "SetLoopVariable"),
    (0x1a, "SMD", 0, "SetMinimumDistance"),
    (0x0a, "SPVFS", 0, "SetPVectorFromStack"),
    (0x02, "SPVTCA", 1, "SetPVectorToAxis"),
    (0x06, "SPVTL", 1, "SetPVectorToLine"),
    (0x76, "SROUND", 0, "SuperRound"),
    (0x10, "SRP0", 0, "SetRefPoint0"),
    (0x11, "SRP1", 0, "SetRefPoint1"),
    (0x12, "SRP2", 0, "SetRefPoint2"),
    (0x1f, "SSW", 0, "SetSingleWidth"),
    (0x1e, "SSWCI", 0, "SetSingleWidthCutIn"),
    (0x61, "SUB", 0, "Subtract"),
    (0x00, "SVTCA", 1, "SetFPVectorToAxis"),
    (0x23, "SWAP", 0, "SwapTopStack"),
    (0x13, "SZP0", 0, "SetZonePointer0"),
    (0x14, "SZP1", 0, "SetZonePointer1"),
    (0x15, "SZP2", 0, "SetZonePointer2"),
    (0x16, "SZPS", 0, "SetZonePointerS"),
    (0x29, "UTP", 0, "UnTouchPt"),
    (0x70, "WCVTF", 0, "WriteCVTInFUnits"),
    (0x44, "WCVTP", 0, "WriteCVTInPixels"),
    (0x42, "WS", 0, "WriteStore"),
];

/// Look up an opcode: (mnemonic, argBits, base opcode, name).
fn lookup(opcode: u8) -> Option<(&'static str, u8, u8, &'static str)> {
    // fontTools checks the stream (push) instructions first.
    for table in [STREAM_INSTRUCTIONS, INSTRUCTIONS] {
        for &(base, mnemonic, arg_bits, name) in table {
            let span = 1u16 << arg_bits;
            if (base as u16..base as u16 + span).contains(&(opcode as u16)) {
                return Some((mnemonic, arg_bits, base, name));
            }
        }
    }
    None
}

/// fontTools.misc.textTools.num2binary for small bit counts.
fn num2binary(value: u8, bits: u8) -> String {
    (0..bits)
        .rev()
        .map(|i| if value >> i & 1 == 1 { '1' } else { '0' })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Ttx,
    Vtt,
}

/// Disassemble bytecode into fontTools ("ttx") assembly, as
/// `str(InstructionStream(program_bytes=...))` does.
pub fn ttx_assembly(bytecode: &[u8]) -> Result<String, Error> {
    assembly(bytecode, Dialect::Ttx)
}

/// Disassemble bytecode into VTT assembly, as
/// `InstructionStream(program_bytes=...).vtt_assembly` does.
pub fn vtt_assembly(bytecode: &[u8]) -> Result<String, Error> {
    assembly(bytecode, Dialect::Vtt)
}

/// What ufo-extractor stores in `public.truetype.instructions` for a program.
pub fn lib_assembly(bytecode: &[u8]) -> Result<String, Error> {
    Ok(format!("\n{}\n", ttx_assembly(bytecode)?))
}

fn assembly(bytecode: &[u8], dialect: Dialect) -> Result<String, Error> {
    let end = "\n";
    let mut asm = String::new();
    let mut indent: i32 = 0;
    let mut pos = 0usize;
    let read_byte = |pos: &mut usize| -> Result<u8, Error> {
        let b = *bytecode.get(*pos).ok_or("truncated instruction stream")?;
        *pos += 1;
        Ok(b)
    };

    while pos < bytecode.len() {
        let opcode = read_byte(&mut pos)?;
        let (mut cmd_name, mut arg_bits, base, name) = lookup(opcode)
            .ok_or_else(|| format!("Illegal opcode 0x{opcode:02x} at offset 0x{pos:04x}."))?;
        if matches!(cmd_name, "EIF" | "ELSE" | "ENDF") {
            indent -= 1;
        }
        let pad = "  ".repeat(indent.max(0) as usize);

        let is_push = matches!(cmd_name, "NPUSHB" | "NPUSHW" | "PUSHB" | "PUSHW");
        let mut args: Vec<String> = Vec::new();
        if is_push {
            let num_args = if cmd_name.starts_with("PUSH") {
                (opcode - base) as usize + 1
            } else {
                read_byte(&mut pos)? as usize
            };
            for _ in 0..num_args {
                if cmd_name.ends_with('B') {
                    args.push(read_byte(&mut pos)?.to_string());
                } else {
                    let hi = read_byte(&mut pos)?;
                    let lo = read_byte(&mut pos)?;
                    args.push(i16::from_be_bytes([hi, lo]).to_string());
                }
            }
            arg_bits = 0; // no bits for push instructions
        }

        let arg_bitstring = if arg_bits == 0 {
            if dialect == Dialect::Ttx {
                " ".to_string()
            } else {
                String::new()
            }
        } else {
            let bits = num2binary(opcode - base, arg_bits);
            match dialect {
                Dialect::Ttx => bits,
                Dialect::Vtt => bitstring_to_mnemonic(cmd_name, &bits)?,
            }
        };

        match dialect {
            Dialect::Ttx => {
                if is_push {
                    let val = if args.len() == 1 { "value" } else { "values" };
                    asm.push_str(&format!(
                        "\n{pad}{cmd_name}[{arg_bitstring}]\t/* {} {val} pushed */",
                        args.len()
                    ));
                } else {
                    asm.push_str(&format!("\n{pad}{cmd_name}[{arg_bitstring}]\t/* {name} */"));
                }
                if !args.is_empty() {
                    asm.push_str(&format!("\n{pad}{}", args.join(" ")));
                }
            }
            Dialect::Vtt => {
                if is_push {
                    cmd_name = "#PUSH";
                    asm.push_str(&format!("{end}{pad}{cmd_name}, {}", args.join(", ")));
                } else if matches!(cmd_name, "JMPR" | "JROF") {
                    // extractor checks for a misspelled "JPMR", so JMPR keeps no args
                    if cmd_name == "JROF" {
                        args = vec!["*".into(), "*".into()];
                    }
                    asm.push_str(&format!("{end}#PUSHON"));
                    asm.push_str(&format!("{end}{pad}{cmd_name}, {}", args.join(", ")));
                    asm.push_str(&format!("{end}#PUSHOFF"));
                } else {
                    asm.push_str(&format!(
                        "{end}{pad}{cmd_name}[{arg_bitstring}]\t/* {name} */"
                    ));
                }
            }
        }

        if matches!(cmd_name, "ELSE" | "FDEF" | "IF") {
            indent += 1;
        }
    }

    let asm = asm.trim();
    Ok(match dialect {
        Dialect::Ttx => asm.to_string(),
        Dialect::Vtt if asm.is_empty() => String::new(),
        Dialect::Vtt => format!("#PUSHOFF{end}{asm}{end}#PUSHON"),
    })
}

fn bitstring_to_mnemonic(cmd_name: &str, bits: &str) -> Result<String, Error> {
    let pick = |zero: &str, one: &str| if bits == "0" { zero } else { one }.to_string();
    Ok(match cmd_name {
        "SVTCA" | "SPVTCA" | "SFVTCA" | "IUP" => pick("Y", "X"),
        "SPVTL" | "SFVTL" | "SDPVTL" => pick("r", "R"),
        "MDAP" | "MIAP" => pick("r", "R"),
        "SHP" | "SHC" | "SHZ" => pick("2", "1"),
        "MSIRP" => pick("m", "M"),
        "GC" | "MD" => pick("N", "O"),
        "ROUND" | "NROUND" => color_mnemonic(bits)?.to_string(),
        "MDRP" | "MIRP" => {
            let b = bits.as_bytes();
            let mut flags = String::new();
            flags.push(if b[0] == b'0' { 'm' } else { 'M' });
            flags.push(if b[1] == b'0' { '<' } else { '>' });
            flags.push(if b[2] == b'0' { 'r' } else { 'R' });
            flags + color_mnemonic(&bits[3..])?
        }
        _ => return Err(format!("no VTT mnemonic for {cmd_name}[{bits}]").into()),
    })
}

fn color_mnemonic(bits: &str) -> Result<&'static str, Error> {
    match bits {
        "00" => Ok("Gr"),
        "01" => Ok("Bl"),
        "10" => Ok("Wh"),
        _ => Err(format!("no VTT distance color for bits {bits}").into()),
    }
}
