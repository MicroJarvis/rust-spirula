//! Self-contained PLY vertex and Gaussian splat readers (P3-F04/P3-F06).
//!
//! ASCII + `binary_little_endian` only. Fixed-size scalar properties; rejects
//! list properties, big-endian, truncated bodies, unknown types, and over-budget
//! files. Gaussian splats use the exact reference 17-property identity and
//! write binary little-endian float32. Pin: `NerfstudioParser.cpp`
//! `PlyVertexReader` / `read_ply_points`, `SplatPly.cpp`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use spirula_core::{CancelToken, ErrorCode, GaussianSplat, Quat, SplatSet, Vec3};

use crate::atomic::{PublishOptions, publish_bytes_validated};
use crate::colmap::budget::{
    MAX_COLMAP_FILE_BYTES, MAX_LOAD_MEMORY_BYTES, MAX_POINTS, ParseCancel, ParseMem, ensure_count,
    err, err_path, prepare_vec_push, try_reserve_elems,
};
use crate::colmap::{ColmapPoint3D, ColmapPoints3D};

const READ_CHUNK: usize = 256 * 1024;

const SPLAT_PROPERTIES: [&str; 17] = [
    "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
    "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlyType {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl PlyType {
    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    fn from_name(s: &str) -> Result<Self, spirula_core::DomainError> {
        Ok(match s {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => {
                return Err(err(
                    ErrorCode::MalformedRecord,
                    "ply_type",
                    format!("PLY: unknown property type {s}"),
                ));
            }
        })
    }

    fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    fn read_le(self, p: &[u8]) -> f64 {
        match self {
            Self::I8 => i8::from_le_bytes([p[0]]) as f64,
            Self::U8 => p[0] as f64,
            Self::I16 => i16::from_le_bytes([p[0], p[1]]) as f64,
            Self::U16 => u16::from_le_bytes([p[0], p[1]]) as f64,
            Self::I32 => i32::from_le_bytes(p[..4].try_into().unwrap()) as f64,
            Self::U32 => u32::from_le_bytes(p[..4].try_into().unwrap()) as f64,
            Self::F32 => f32::from_le_bytes(p[..4].try_into().unwrap()) as f64,
            Self::F64 => f64::from_le_bytes(p[..8].try_into().unwrap()),
        }
    }
}

#[derive(Clone, Debug)]
struct PlyProp {
    name: String,
    ty: PlyType,
}

#[derive(Clone, Debug)]
struct PlyElement {
    name: String,
    count: u64,
    props: Vec<PlyProp>,
    has_list: bool,
}

impl PlyElement {
    fn stride(&self) -> usize {
        self.props.iter().map(|p| p.ty.size()).sum()
    }
}

fn open_err(path: &Path, e: std::io::Error) -> spirula_core::DomainError {
    let code = if e.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::NotFound
    } else if e.kind() == std::io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::ReadFailed
    };
    err_path(
        code,
        &path.display().to_string(),
        format!("PLY: cannot open: {e}"),
    )
}

fn split_ws(s: &str) -> Vec<&str> {
    s.split([' ', '\t']).filter(|t| !t.is_empty()).collect()
}

fn read_capped(
    path: &Path,
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>, spirula_core::DomainError> {
    let meta = std::fs::metadata(path).map_err(|e| open_err(path, e))?;
    let len = meta.len();
    if len > MAX_COLMAP_FILE_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "file_size",
            format!(
                "PLY file {} exceeds budget {MAX_COLMAP_FILE_BYTES} bytes",
                path.display()
            ),
        ));
    }
    mem.charge(len, "ply_file")?;
    let mut charged = len;

    let mut file = File::open(path).map_err(|e| open_err(path, e))?;
    let mut data = Vec::new();
    if len > 0 {
        data.try_reserve(len as usize).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "ply_file",
                format!(
                    "PLY file {} reservation of {len} bytes failed",
                    path.display()
                ),
            )
        })?;
    }

    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;
    let mut buf = [0u8; READ_CHUNK];
    let mut read_total = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &path.display().to_string(),
                format!("PLY: cannot read: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        read_total = read_total.saturating_add(n as u64);
        if read_total > MAX_COLMAP_FILE_BYTES {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "file_size",
                format!(
                    "PLY file {} grew past budget {MAX_COLMAP_FILE_BYTES} bytes during read",
                    path.display()
                ),
            ));
        }
        if read_total > charged {
            mem.charge(read_total - charged, "ply_file")?;
            charged = read_total;
        }
        data.extend_from_slice(&buf[..n]);
        cp.tick(n)?;
    }
    Ok(data)
}

fn header_line<'a>(
    bytes: &'a [u8],
    pos: &mut usize,
    what: &str,
) -> Result<&'a str, spirula_core::DomainError> {
    let rest = &bytes[*pos..];
    let nl = rest.iter().position(|&b| b == b'\n').ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "ply_header",
            format!("PLY: truncated header in {what}"),
        )
    })?;
    let mut line = &rest[..nl];
    if let Some(b'\r') = line.last().copied() {
        line = &line[..line.len() - 1];
    }
    *pos += nl + 1;
    std::str::from_utf8(line).map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            "ply_header",
            format!("PLY: non-UTF8 header in {what}"),
        )
    })
}

struct PlyVertexView<'a> {
    bytes: &'a [u8],
    data_off: usize,
    count: u64,
    binary: bool,
    names: Vec<String>,
    types: Vec<PlyType>,
}

impl<'a> PlyVertexView<'a> {
    fn parse(
        bytes: &'a [u8],
        what: &str,
        mem: &mut ParseMem,
        cancel: Option<&CancelToken>,
    ) -> Result<Self, spirula_core::DomainError> {
        let mut cp = ParseCancel::new(cancel);
        cp.poll_now()?;
        let mut pos = 0usize;
        if header_line(bytes, &mut pos, what)? != "ply" {
            return Err(err(
                ErrorCode::MalformedRecord,
                "ply_magic",
                format!("PLY: missing magic in {what}"),
            ));
        }
        let mut binary = false;
        let mut elements: Vec<PlyElement> = Vec::new();
        loop {
            cp.tick(1)?;
            let line = header_line(bytes, &mut pos, what)?;
            let tok = split_ws(line);
            if tok.is_empty() {
                continue;
            }
            match tok[0] {
                "comment" | "obj_info" => continue,
                "format" => {
                    if tok.len() < 2 {
                        return Err(err(
                            ErrorCode::MalformedRecord,
                            "ply_format",
                            "PLY: bad format line",
                        ));
                    }
                    match tok[1] {
                        "ascii" => binary = false,
                        "binary_little_endian" => binary = true,
                        other => {
                            return Err(err(
                                ErrorCode::UnsupportedFormat,
                                "ply_format",
                                format!(
                                    "PLY: unsupported format {other} (big-endian not supported)"
                                ),
                            ));
                        }
                    }
                }
                "element" => {
                    if tok.len() < 3 {
                        return Err(err(
                            ErrorCode::MalformedRecord,
                            "ply_element",
                            "PLY: bad element line",
                        ));
                    }
                    let count: u64 = tok[2].parse().map_err(|_| {
                        err(
                            ErrorCode::MalformedRecord,
                            "ply_element",
                            format!("PLY: bad element count {}", tok[2]),
                        )
                    })?;
                    mem.charge_str_bytes(tok[1].len(), "ply_element")?;
                    prepare_vec_push(&mut elements, mem, "ply_element")?;
                    elements.push(PlyElement {
                        name: tok[1].to_owned(),
                        count,
                        props: Vec::new(),
                        has_list: false,
                    });
                }
                "property" => {
                    let el = elements.last_mut().ok_or_else(|| {
                        err(
                            ErrorCode::MalformedRecord,
                            "ply_property",
                            "PLY: property before element",
                        )
                    })?;
                    if tok.len() < 2 {
                        return Err(err(
                            ErrorCode::MalformedRecord,
                            "ply_property",
                            "PLY: bad property line",
                        ));
                    }
                    if tok[1] == "list" {
                        el.has_list = true;
                        continue;
                    }
                    if tok.len() < 3 {
                        return Err(err(
                            ErrorCode::MalformedRecord,
                            "ply_property",
                            "PLY: bad property line",
                        ));
                    }
                    mem.charge_str_bytes(tok[2].len(), "ply_property")?;
                    prepare_vec_push(&mut el.props, mem, "ply_property")?;
                    el.props.push(PlyProp {
                        name: tok[2].to_owned(),
                        ty: PlyType::from_name(tok[1])?,
                    });
                }
                "end_header" => break,
                _ => {}
            }
        }

        let mut cursor = pos;
        for el in &elements {
            if el.name != "vertex" {
                if el.has_list {
                    return Err(err(
                        ErrorCode::UnsupportedFormat,
                        "ply_list",
                        format!("PLY: list property before vertex element not supported in {what}"),
                    ));
                }
                if binary {
                    let stride = el.stride() as u64;
                    let need = el.count.checked_mul(stride).ok_or_else(|| {
                        err(
                            ErrorCode::ValueOutOfRange,
                            "ply_skip",
                            "PLY: element size overflow",
                        )
                    })?;
                    let rem = (bytes.len() - cursor) as u64;
                    if rem < need {
                        return Err(err(
                            ErrorCode::MalformedRecord,
                            "ply_body",
                            format!("PLY: truncated {what}"),
                        ));
                    }
                    cursor += need as usize;
                    cp.tick(need.min(usize::MAX as u64) as usize)?;
                } else {
                    for _ in 0..el.count {
                        cp.tick(1)?;
                        let rest = &bytes[cursor..];
                        let nl = rest.iter().position(|&b| b == b'\n').ok_or_else(|| {
                            err(
                                ErrorCode::MalformedRecord,
                                "ply_body",
                                format!("PLY: truncated {what}"),
                            )
                        })?;
                        cursor += nl + 1;
                    }
                }
                continue;
            }
            if el.has_list {
                return Err(err(
                    ErrorCode::UnsupportedFormat,
                    "ply_list",
                    format!("PLY: list property in vertex element of {what}"),
                ));
            }
            if binary {
                let stride = el.stride() as u64;
                let need = el.count.checked_mul(stride).ok_or_else(|| {
                    err(
                        ErrorCode::ValueOutOfRange,
                        "ply_vertex",
                        "PLY: vertex size overflow",
                    )
                })?;
                if ((bytes.len() - cursor) as u64) < need {
                    return Err(err(
                        ErrorCode::MalformedRecord,
                        "ply_body",
                        format!("PLY: truncated {what}"),
                    ));
                }
            }
            // Charge name/type column tables before cloning out of header props.
            for p in &el.props {
                mem.charge_str_bytes(p.name.len(), "ply_vertex_cols")?;
            }
            mem.charge_elems::<String>(el.props.len(), "ply_vertex_cols")?;
            mem.charge_elems::<PlyType>(el.props.len(), "ply_vertex_cols")?;
            return Ok(Self {
                bytes,
                data_off: cursor,
                count: el.count,
                binary,
                names: el.props.iter().map(|p| p.name.clone()).collect(),
                types: el.props.iter().map(|p| p.ty).collect(),
            });
        }
        Err(err(
            ErrorCode::MalformedRecord,
            "ply_vertex",
            format!("PLY: no vertex element in {what}"),
        ))
    }

    fn column(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    fn is_float_col(&self, col: usize) -> bool {
        self.types[col].is_float()
    }
}

fn parse_ascii_f64(p: &mut &[u8], what: &str) -> Result<f64, spirula_core::DomainError> {
    while let Some(&b) = p.first() {
        if b.is_ascii_whitespace() {
            *p = &p[1..];
        } else {
            break;
        }
    }
    if p.is_empty() {
        return Err(err(
            ErrorCode::MalformedRecord,
            "ply_ascii",
            format!("PLY: short ascii row in {what}"),
        ));
    }
    // Token-bounded: find whitespace (or EOF) on bytes first, then UTF-8/parse
    // only that slice — never `from_utf8` the entire remaining body (O(n²)).
    let end = p
        .iter()
        .position(|&b| b.is_ascii_whitespace())
        .unwrap_or(p.len());
    if end == 0 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "ply_ascii",
            format!("PLY: short ascii row in {what}"),
        ));
    }
    let tok = std::str::from_utf8(&p[..end]).map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            "ply_ascii",
            format!("PLY: non-UTF8 ascii token in {what}"),
        )
    })?;
    let v: f64 = tok.parse().map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            "ply_ascii",
            format!("PLY: bad ascii number `{tok}` in {what}"),
        )
    })?;
    *p = &p[end..];
    Ok(v)
}

/// Read PLY vertices into [`ColmapPoints3D`].
///
/// Requires scalar `x`/`y`/`z`. RGB (`red`/`green`/`blue`) is optional; missing
/// channels default to `255`. Float RGB is treated as 0..1 and scaled ×255.
pub fn read_ply_points(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<ColmapPoints3D, spirula_core::DomainError> {
    read_ply_points_budgeted(path, cancel, MAX_LOAD_MEMORY_BYTES)
}

/// Same as [`read_ply_points`] with an explicit cumulative memory budget.
pub fn read_ply_points_budgeted(
    path: &Path,
    cancel: Option<&CancelToken>,
    memory_budget_bytes: u64,
) -> Result<ColmapPoints3D, spirula_core::DomainError> {
    let mut mem = ParseMem::with_cap(memory_budget_bytes);
    read_ply_points_mem(path, cancel, &mut mem)
}

/// Read PLY vertices charging a shared [`ParseMem`] (parent load keeps the bytes).
pub(crate) fn read_ply_points_mem(
    path: &Path,
    cancel: Option<&CancelToken>,
    mem: &mut ParseMem,
) -> Result<ColmapPoints3D, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;
    let bytes = read_capped(path, mem, cancel)?;
    cp.poll_now()?;
    let what = path.display().to_string();
    let view = PlyVertexView::parse(&bytes, &what, mem, cancel)?;
    ensure_count(view.count, MAX_POINTS, "ply_points")?;
    cp.tick(1)?;

    let ix = view.column("x").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "ply_xyz",
            format!("PLY: vertex element missing x/y/z in {what}"),
        )
    })?;
    let iy = view.column("y").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "ply_xyz",
            format!("PLY: vertex element missing x/y/z in {what}"),
        )
    })?;
    let iz = view.column("z").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "ply_xyz",
            format!("PLY: vertex element missing x/y/z in {what}"),
        )
    })?;
    let ir = view.column("red");
    let ig = view.column("green");
    let ib = view.column("blue");

    let rgb_scale = [
        ir.map(|c| if view.is_float_col(c) { 255.0 } else { 1.0 })
            .unwrap_or(1.0),
        ig.map(|c| if view.is_float_col(c) { 255.0 } else { 1.0 })
            .unwrap_or(1.0),
        ib.map(|c| if view.is_float_col(c) { 255.0 } else { 1.0 })
            .unwrap_or(1.0),
    ];

    let n = view.count as usize;
    let mut points = Vec::new();
    try_reserve_elems(&mut points, n, mem, "ply_points")?;

    let nprops = view.types.len();
    let mut offsets = Vec::new();
    try_reserve_elems(&mut offsets, nprops, mem, "ply_offsets")?;
    let mut stride = 0usize;
    for ty in &view.types {
        offsets.push(stride);
        stride += ty.size();
    }

    // One ASCII row scratch buffer for the whole vertex loop (not per-row alloc).
    let mut ascii_vals = Vec::new();
    if !view.binary {
        try_reserve_elems(&mut ascii_vals, nprops, mem, "ply_ascii_vals")?;
        ascii_vals.resize(nprops, 0.0f64);
    }

    if view.binary {
        let body = &view.bytes[view.data_off..];
        for i in 0..n {
            cp.tick(1)?;
            let row = &body[i * stride..];
            let x = view.types[ix].read_le(&row[offsets[ix]..]);
            let y = view.types[iy].read_le(&row[offsets[iy]..]);
            let z = view.types[iz].read_le(&row[offsets[iz]..]);
            if ![x, y, z].iter().all(|v| v.is_finite()) {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "xyz",
                    format!("PLY: non-finite xyz at vertex {i} in {what}"),
                ));
            }
            let mut rgb = [255u8; 3];
            if let (Some(r), Some(g), Some(b)) = (ir, ig, ib) {
                let rv = view.types[r].read_le(&row[offsets[r]..]) * rgb_scale[0];
                let gv = view.types[g].read_le(&row[offsets[g]..]) * rgb_scale[1];
                let bv = view.types[b].read_le(&row[offsets[b]..]) * rgb_scale[2];
                rgb = [
                    rv.clamp(0.0, 255.0) as u8,
                    gv.clamp(0.0, 255.0) as u8,
                    bv.clamp(0.0, 255.0) as u8,
                ];
            }
            points.push(ColmapPoint3D {
                point3d_id: i as u64,
                xyz: [x, y, z],
                rgb,
                error: 0.0,
                track: Vec::new(),
            });
        }
    } else {
        let mut p = &view.bytes[view.data_off..];
        for i in 0..n {
            cp.tick(1)?;
            for slot in ascii_vals.iter_mut() {
                *slot = parse_ascii_f64(&mut p, &what)?;
            }
            let vals = &ascii_vals;
            let x = vals[ix];
            let y = vals[iy];
            let z = vals[iz];
            if ![x, y, z].iter().all(|v| v.is_finite()) {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "xyz",
                    format!("PLY: non-finite xyz at vertex {i} in {what}"),
                ));
            }
            let mut rgb = [255u8; 3];
            if let (Some(r), Some(g), Some(b)) = (ir, ig, ib) {
                let rv = vals[r] * rgb_scale[0];
                let gv = vals[g] * rgb_scale[1];
                let bv = vals[b] * rgb_scale[2];
                rgb = [
                    rv.clamp(0.0, 255.0) as u8,
                    gv.clamp(0.0, 255.0) as u8,
                    bv.clamp(0.0, 255.0) as u8,
                ];
            }
            points.push(ColmapPoint3D {
                point3d_id: i as u64,
                xyz: [x, y, z],
                rgb,
                error: 0.0,
                track: Vec::new(),
            });
        }
    }

    Ok(ColmapPoints3D { points })
}

fn splat_error(field: &str, message: impl Into<String>) -> spirula_core::DomainError {
    err(ErrorCode::MalformedRecord, field, message)
}

fn parse_splat_bytes(
    bytes: &[u8],
    what: &str,
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<SplatSet, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;
    let view = PlyVertexView::parse(bytes, what, mem, cancel)?;
    ensure_count(view.count, MAX_POINTS, "ply_splats")?;
    if view.names.len() != SPLAT_PROPERTIES.len()
        || view
            .names
            .iter()
            .zip(SPLAT_PROPERTIES)
            .any(|(actual, expected)| actual != expected)
    {
        return Err(splat_error(
            "splat_properties",
            format!(
                "PLY: vertex properties must be exactly {:?} in {what}",
                SPLAT_PROPERTIES
            ),
        ));
    }
    if view.types.iter().any(|ty| !ty.is_float()) {
        return Err(splat_error(
            "splat_properties",
            format!("PLY: splat properties must be scalar floating point values in {what}"),
        ));
    }

    let n = view.count as usize;
    let mut splats = Vec::new();
    try_reserve_elems(&mut splats, n, mem, "ply_splats")?;
    let mut offsets = Vec::new();
    try_reserve_elems(&mut offsets, view.types.len(), mem, "ply_splat_offsets")?;
    let mut stride = 0usize;
    for ty in &view.types {
        offsets.push(stride);
        stride = stride.checked_add(ty.size()).ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "ply_splat_stride",
                "PLY: splat stride overflow",
            )
        })?;
    }

    let mut rows = Vec::new();
    if view.binary {
        let body = &view.bytes[view.data_off..];
        let expected = n.checked_mul(stride).ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "ply_splat_body",
                "PLY: splat body size overflow",
            )
        })?;
        if body.len() != expected {
            return Err(splat_error(
                "ply_splat_body",
                format!("PLY: trailing or truncated splat body in {what}"),
            ));
        }
        for i in 0..n {
            cp.tick(1)?;
            let row = &body[i * stride..];
            rows.push(std::array::from_fn(|j| {
                view.types[j].read_le(&row[offsets[j]..])
            }));
        }
    } else {
        let mut p = &view.bytes[view.data_off..];
        for _ in 0..n {
            cp.tick(1)?;
            let mut row = [0.0; 17];
            for value in &mut row {
                *value = parse_ascii_f64(&mut p, what)?;
            }
            rows.push(row);
        }
        if p.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err(splat_error(
                "ply_ascii",
                format!("PLY: extra ascii tokens after splat rows in {what}"),
            ));
        }
    }

    for (i, row) in rows.into_iter().enumerate() {
        if row.iter().any(|value| !value.is_finite()) {
            return Err(spirula_core::DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                format!("PLY: non-finite splat value at vertex {i} in {what}"),
            )
            .expect("message"));
        }
        let splat = GaussianSplat::try_new(
            Vec3::try_new(row[0], row[1], row[2])?,
            Quat::try_from_wxyz(row[13], row[14], row[15], row[16])?,
            Vec3::try_new(row[10], row[11], row[12])?,
            row[9],
            Vec3::try_new(row[6], row[7], row[8])?,
        )?;
        splats.push(splat);
    }
    SplatSet::try_from_splats(splats)
}

/// Read a Gaussian splat PLY in the reference property order.
pub fn read_ply_splats(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<SplatSet, spirula_core::DomainError> {
    read_ply_splats_budgeted(path, cancel, MAX_LOAD_MEMORY_BYTES)
}

/// Read a Gaussian splat PLY with a cumulative memory budget.
pub fn read_ply_splats_budgeted(
    path: &Path,
    cancel: Option<&CancelToken>,
    memory_budget_bytes: u64,
) -> Result<SplatSet, spirula_core::DomainError> {
    let mut mem = ParseMem::with_cap(memory_budget_bytes);
    let bytes = read_capped(path, &mut mem, cancel)?;
    parse_splat_bytes(&bytes, &path.display().to_string(), &mut mem, cancel)
}

fn write_f32(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&(value as f32).to_le_bytes());
}

fn splat_bytes(
    set: &SplatSet,
    cancel: Option<&CancelToken>,
    max_bytes: u64,
) -> Result<Vec<u8>, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;
    let mut out = Vec::new();
    out.extend_from_slice(
        b"ply\nformat binary_little_endian 1.0\ncomment spirula Gaussian splat PLY\nelement vertex ",
    );
    out.extend_from_slice(set.len().to_string().as_bytes());
    out.extend_from_slice(b"\n");
    for property in SPLAT_PROPERTIES {
        out.extend_from_slice(b"property float ");
        out.extend_from_slice(property.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"end_header\n");
    let body_bytes = set
        .len()
        .checked_mul(SPLAT_PROPERTIES.len() * 4)
        .ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "ply_splat_size",
                "splat output size overflow",
            )
        })?;
    let total_bytes = out.len().checked_add(body_bytes).ok_or_else(|| {
        err(
            ErrorCode::ValueOutOfRange,
            "ply_splat_size",
            "splat output size overflow",
        )
    })?;
    if total_bytes as u64 > max_bytes {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "ply_splat_size",
            format!("splat output requires {total_bytes} bytes, budget is {max_bytes}"),
        ));
    }
    out.try_reserve(body_bytes).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            "ply_splat_size",
            "splat output reservation failed",
        )
    })?;
    for splat in set.splats() {
        cp.tick(1)?;
        let mean = splat.mean().to_array();
        let mut raw = Vec::with_capacity(17);
        raw.extend_from_slice(&mean);
        raw.extend_from_slice(&[0.0; 3]);
        raw.extend_from_slice(&splat.dc().to_array());
        raw.push(splat.opacity_logit());
        raw.extend_from_slice(&splat.scale_log().to_array());
        raw.extend_from_slice(&splat.rotation().wxyz());
        if raw.iter().any(|value| !(*value as f32).is_finite()) {
            return Err(spirula_core::DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                "splat value cannot be represented as finite float32",
            )
            .expect("message"));
        }
        for value in mean {
            write_f32(&mut out, value);
        }
        for _ in 0..3 {
            write_f32(&mut out, 0.0);
        }
        for value in splat.dc().to_array() {
            write_f32(&mut out, value);
        }
        write_f32(&mut out, splat.opacity_logit());
        for value in splat.scale_log().to_array() {
            write_f32(&mut out, value);
        }
        for value in splat.rotation().wxyz() {
            write_f32(&mut out, value);
        }
    }
    Ok(out)
}

/// Atomically write a reference-compatible binary little-endian splat PLY.
pub fn write_ply_splats(
    path: &Path,
    set: &SplatSet,
    cancel: Option<&CancelToken>,
) -> Result<(), spirula_core::DomainError> {
    write_ply_splats_budgeted(path, set, cancel, MAX_COLMAP_FILE_BYTES)
}

/// Atomically write a splat PLY with an explicit output byte budget.
pub fn write_ply_splats_budgeted(
    path: &Path,
    set: &SplatSet,
    cancel: Option<&CancelToken>,
    max_bytes: u64,
) -> Result<(), spirula_core::DomainError> {
    let bytes = splat_bytes(set, cancel, max_bytes)?;
    let mut options = PublishOptions::default();
    if let Some(token) = cancel {
        options = options.with_cancel(token);
    }
    publish_bytes_validated(path, &bytes, options, |payload| {
        let mut mem = ParseMem::with_cap(MAX_LOAD_MEMORY_BYTES);
        parse_splat_bytes(payload, "published splat PLY", &mut mem, None).map(|_| ())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_harness::TempHarness;
    use spirula_core::ErrorCode;

    #[test]
    fn ascii_xyz_rgb() {
        let h = TempHarness::new().unwrap();
        let p = h.path().join("a.ply");
        std::fs::write(
            &p,
            b"ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n1 2 3 10 20 30\n4 5 6 40 50 60\n",
        )
        .unwrap();
        let pts = read_ply_points(&p, None).unwrap();
        assert_eq!(pts.num(), 2);
        assert_eq!(pts.points[0].xyz, [1.0, 2.0, 3.0]);
        assert_eq!(pts.points[0].rgb, [10, 20, 30]);
        assert_eq!(pts.points[1].rgb, [40, 50, 60]);
    }

    #[test]
    fn binary_le_xyz_optional_rgb_defaults() {
        let h = TempHarness::new().unwrap();
        let p = h.path().join("b.ply");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n",
        );
        bytes.extend_from_slice(&1.5f32.to_le_bytes());
        bytes.extend_from_slice(&(-2.5f32).to_le_bytes());
        bytes.extend_from_slice(&3.25f32.to_le_bytes());
        std::fs::write(&p, &bytes).unwrap();
        let pts = read_ply_points(&p, None).unwrap();
        assert_eq!(pts.num(), 1);
        assert!((pts.points[0].xyz[0] - 1.5).abs() < 1e-6);
        assert_eq!(pts.points[0].rgb, [255, 255, 255]);
    }

    #[test]
    fn rejects_big_endian_list_truncated() {
        let h = TempHarness::new().unwrap();
        let be = h.path().join("be.ply");
        std::fs::write(
            &be,
            b"ply\nformat binary_big_endian 1.0\nelement vertex 0\nend_header\n",
        )
        .unwrap();
        assert_eq!(
            read_ply_points(&be, None).unwrap_err().code(),
            ErrorCode::UnsupportedFormat
        );

        let list = h.path().join("list.ply");
        std::fs::write(
            &list,
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty list uchar int vertex_indices\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 1 2 3\n",
        )
        .unwrap();
        assert_eq!(
            read_ply_points(&list, None).unwrap_err().code(),
            ErrorCode::UnsupportedFormat
        );

        let trunc = h.path().join("trunc.ply");
        std::fs::write(
            &trunc,
            b"ply\nformat binary_little_endian 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nend_header\n",
        )
        .unwrap();
        // only header — body truncated
        assert_eq!(
            read_ply_points(&trunc, None).unwrap_err().code(),
            ErrorCode::MalformedRecord
        );
    }

    #[test]
    fn budget_rejects_before_alloc() {
        let h = TempHarness::new().unwrap();
        let p = h.path().join("big.ply");
        let mut body = String::from(
            "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nend_header\n",
        );
        for i in 0..3 {
            body.push_str(&format!("{i}.0 {i}.0 {i}.0\n"));
        }
        std::fs::write(&p, body).unwrap();
        let err = read_ply_points_budgeted(&p, None, 8).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }
}
