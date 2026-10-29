//! Train / eval / validation splits (`DatasetCommon.cpp:183-258`).
//!
//! # Index domain
//!
//! [`SplitPlan::train`] / [`SplitPlan::eval`] / [`SplitPlan::validation`] are
//! indices into the **emitted** frame list (`SplitPlan::emit` → sorted frames
//! that become [`crate::colmap::ParsedDataset::frames`]). Validation is carved
//! **once** from the true train keep-set; load must not re-apply
//! [`assign_val_split`].
//!
//! Global membership (which sorted names are train vs eval) is decided by
//! [`train_keep_set`] / [`eval_keep_set`]. Local indices after emit remap follow
//! DataManager: `split=eval` loads the eval keep as the only frames and labels
//! them all as local train with `validation=0` (the eval set is already held
//! out).

use std::path::Path;

use spirula_core::ErrorCode;

use super::budget::err;

/// NumPy-style `linspace(0, n-1, count, dtype=int)` (truncate toward zero).
pub fn linspace_indices(n: i64, count: i64) -> Vec<i64> {
    let mut out = Vec::new();
    if count <= 0 || n <= 0 {
        return out;
    }
    let _ = out.try_reserve(count as usize);
    for k in 0..count {
        if count == 1 {
            out.push(0);
            continue;
        }
        if k == count - 1 {
            out.push(n - 1);
            continue;
        }
        let idx = ((k as f64) * ((n - 1) as f64) / ((count - 1) as f64)) as i64;
        out.push(idx);
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitConfig {
    pub eval_mode: String,
    pub train_split_fraction: f32,
    pub eval_interval: i32,
    /// `"train"` | `"eval"` | `"all"` — `"all"` emits every frame (Rust summary).
    pub split: String,
    pub validation_fraction: f32,
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            eval_mode: "all".into(),
            train_split_fraction: 0.9,
            eval_interval: 8,
            split: "train".into(),
            validation_fraction: 0.0,
        }
    }
}

/// True training keep-set from `eval_mode` (global indices into the sorted
/// name list). Never mixes eval-complement frames for `filename` / `interval`
/// / `fraction`. Empty keep → error. Does **not** apply validation.
pub fn train_keep_set(
    n: i64,
    names: &[String],
    cfg: &SplitConfig,
) -> Result<Vec<i64>, spirula_core::DomainError> {
    if n < 0 {
        return Err(err(ErrorCode::InvalidArgument, "n", "negative frame count"));
    }
    if names.len() as i64 != n {
        return Err(err(
            ErrorCode::InvalidArgument,
            "names",
            format!("names len {} != n {n}", names.len()),
        ));
    }
    let mut keep = Vec::new();
    match cfg.eval_mode.as_str() {
        "all" => {
            keep.extend(0..n);
        }
        "fraction" => {
            let num_train = ((n as f64) * f64::from(cfg.train_split_fraction)).ceil() as i64;
            let mut flag = vec![false; n as usize];
            for idx in linspace_indices(n, num_train) {
                if (0..n).contains(&idx) {
                    flag[idx as usize] = true;
                }
            }
            for (i, f) in flag.iter().enumerate() {
                if *f {
                    keep.push(i as i64);
                }
            }
        }
        "interval" => {
            for i in 0..n {
                if cfg.eval_interval <= 0 || i % i64::from(cfg.eval_interval) != 0 {
                    keep.push(i);
                }
            }
        }
        "filename" => {
            for (i, name) in names.iter().enumerate() {
                let base = Path::new(name)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name.as_str());
                if base.contains("train") {
                    keep.push(i as i64);
                } else if !base.contains("eval") {
                    return Err(err(
                        ErrorCode::InvalidArgument,
                        "eval_mode",
                        format!(
                            "eval_mode=filename requires 'train'/'eval' in every image name; got {name}"
                        ),
                    ));
                }
            }
        }
        other => {
            return Err(err(
                ErrorCode::InvalidArgument,
                "eval_mode",
                format!("unknown eval_mode {other}"),
            ));
        }
    }
    if keep.is_empty() {
        return Err(err(
            ErrorCode::InvalidArgument,
            "split",
            "eval_mode split left no training images",
        ));
    }
    Ok(keep)
}

/// Eval keep-set in the global sorted-name domain.
///
/// - `eval_mode=all`: coincides with every frame (reference coincidence).
/// - otherwise: complement of the true train keep-set (may be empty — legal).
fn eval_keep_set(n: i64, train_keep: &[i64], eval_mode: &str) -> Vec<i64> {
    if eval_mode == "all" {
        return (0..n).collect();
    }
    let mut is_train = vec![false; n as usize];
    for i in train_keep {
        if (0..n).contains(i) {
            is_train[*i as usize] = true;
        }
    }
    let mut other = Vec::new();
    for i in 0..n {
        if !is_train[i as usize] {
            other.push(i);
        }
    }
    other
}

/// Train keep-set, then optionally return eval complement (`train_subset`).
///
/// Prefer [`compute_splits`] for load: this helper does not apply validation and
/// returns **global** indices into `names`, not the emitted-frame domain.
pub fn train_subset(
    n: i64,
    names: &[String],
    cfg: &SplitConfig,
) -> Result<Vec<i64>, spirula_core::DomainError> {
    let keep = train_keep_set(n, names, cfg)?;
    match cfg.split.as_str() {
        "eval" => Ok(eval_keep_set(n, &keep, &cfg.eval_mode)),
        "train" => Ok(keep),
        "all" => Ok((0..n).collect()),
        other => Err(err(
            ErrorCode::InvalidArgument,
            "split",
            format!("unknown split '{other}' (expected 'train', 'eval', or 'all')"),
        )),
    }
}

/// Validation hold-out over a contiguous local domain of length `n`
/// (`assign_val_split`). Local indices `0..n` map onto whatever keep-set the
/// caller passes in.
pub fn assign_val_split(
    n: i64,
    validation_fraction: f32,
) -> Result<(Vec<usize>, Vec<usize>), spirula_core::DomainError> {
    if n < 0 {
        return Err(err(ErrorCode::InvalidArgument, "n", "negative frame count"));
    }
    let mut is_val = vec![false; n as usize];
    if validation_fraction > 0.0 && n > 1 {
        let num_train = ((n as f64) * (1.0 - f64::from(validation_fraction))).ceil() as i64;
        let mut is_train = vec![false; n as usize];
        for idx in linspace_indices(n, num_train) {
            if (0..n).contains(&idx) {
                is_train[idx as usize] = true;
            }
        }
        for i in 0..n as usize {
            is_val[i] = !is_train[i];
        }
    }
    let mut train = Vec::new();
    let mut val = Vec::new();
    for (i, is_v) in is_val.iter().enumerate() {
        if *is_v {
            val.push(i);
        } else {
            train.push(i);
        }
    }
    Ok((train, val))
}

/// Full split plan: which global frames to emit, plus train/eval/validation
/// indices into that emitted list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitPlan {
    /// Global indices (into the sorted name/frame list) to emit, in order.
    pub emit: Vec<usize>,
    /// Indices into [`Self::emit`].
    pub train: Vec<usize>,
    pub eval: Vec<usize>,
    pub validation: Vec<usize>,
}

fn remap_into_emitted(emitted: &[usize], global: &[usize]) -> Vec<usize> {
    let mut pos = vec![None; emitted.iter().copied().max().map(|m| m + 1).unwrap_or(0)];
    for (local, &g) in emitted.iter().enumerate() {
        if g >= pos.len() {
            pos.resize(g + 1, None);
        }
        pos[g] = Some(local);
    }
    global
        .iter()
        .filter_map(|&g| pos.get(g).and_then(|p| *p))
        .collect()
}

/// Compute the emit list plus train / eval / validation in the **emitted**
/// domain. Validation is applied once to the true train keep-set.
///
/// # Index domains
///
/// - **Global membership** (`train_keep` / `eval_keep`): indices into the
///   sorted name list before emit filtering.
/// - **Local indices** ([`SplitPlan::train`] / [`SplitPlan::eval`] /
///   [`SplitPlan::validation`]): indices into [`SplitPlan::emit`].
///
/// - `all`: emit every sorted frame; train/eval/val remapped from global.
/// - `train`: emit original train keep; train/val remapped; eval empty.
/// - `eval`: emit eval keep (global membership). C++ DataManager eval-only
///   (`ColmapParser.cpp:955-959`) forces `validation_fraction=0` and treats
///   every selected frame as local train — so local `train = 0..emit.len`,
///   `validation` empty, and local `eval` empty (membership already applied
///   by emit selection).
pub fn compute_splits(
    names: &[String],
    cfg: &SplitConfig,
) -> Result<SplitPlan, spirula_core::DomainError> {
    let n = names.len() as i64;
    match cfg.split.as_str() {
        "train" | "eval" | "all" => {}
        other => {
            return Err(err(
                ErrorCode::InvalidArgument,
                "split",
                format!("unknown split '{other}' (expected 'train', 'eval', or 'all')"),
            ));
        }
    }

    let train_keep = train_keep_set(n, names, cfg)?;
    let eval_keep = eval_keep_set(n, &train_keep, &cfg.eval_mode);

    // Validation once on the true train keep (never on eval / never twice).
    // Eval-only loads force validation_fraction=0 (C++ pin).
    let val_frac = if cfg.split == "eval" {
        0.0
    } else {
        cfg.validation_fraction
    };
    let (train_local, val_local) = assign_val_split(train_keep.len() as i64, val_frac)?;
    let train_global: Vec<usize> = train_local
        .iter()
        .map(|i| train_keep[*i] as usize)
        .collect();
    let val_global: Vec<usize> = val_local.iter().map(|i| train_keep[*i] as usize).collect();
    let eval_global: Vec<usize> = eval_keep.iter().map(|i| *i as usize).collect();

    match cfg.split.as_str() {
        "all" => Ok(SplitPlan {
            emit: (0..names.len()).collect(),
            train: train_global,
            eval: eval_global,
            validation: val_global,
        }),
        "train" => {
            let emit: Vec<usize> = train_keep.iter().map(|i| *i as usize).collect();
            Ok(SplitPlan {
                train: remap_into_emitted(&emit, &train_global),
                eval: Vec::new(),
                validation: remap_into_emitted(&emit, &val_global),
                emit,
            })
        }
        "eval" => {
            let emit = eval_global;
            // Local train = all selected frames; validation forced 0 above.
            let local_train: Vec<usize> = (0..emit.len()).collect();
            Ok(SplitPlan {
                emit,
                train: local_train,
                eval: Vec::new(),
                validation: Vec::new(),
            })
        }
        _ => unreachable!("validated above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_eval_ok_empty_train_err() {
        let names: Vec<String> = (0..4).map(|i| format!("f{i}.png")).collect();
        let cfg = SplitConfig {
            eval_mode: "fraction".into(),
            train_split_fraction: 1.0,
            split: "eval".into(),
            ..Default::default()
        };
        let eval = train_subset(4, &names, &cfg).unwrap();
        assert!(eval.is_empty());

        let bad = SplitConfig {
            eval_mode: "filename".into(),
            split: "train".into(),
            ..Default::default()
        };
        let names2 = vec!["a.png".into()];
        assert!(train_subset(1, &names2, &bad).is_err());
    }

    #[test]
    fn interval_mode() {
        let names: Vec<String> = (0..8).map(|i| format!("{i}.png")).collect();
        let cfg = SplitConfig {
            eval_mode: "interval".into(),
            eval_interval: 4,
            split: "train".into(),
            ..Default::default()
        };
        let keep = train_subset(8, &names, &cfg).unwrap();
        assert_eq!(keep, vec![1, 2, 3, 5, 6, 7]);
    }

    #[test]
    fn filename_all_does_not_mark_eval_as_train() {
        // Probe counterexample: sorted [img_eval_b, img_train_a].
        let names = vec!["img_eval_b.png".into(), "img_train_a.png".into()];
        let cfg = SplitConfig {
            eval_mode: "filename".into(),
            split: "all".into(),
            validation_fraction: 0.0,
            ..Default::default()
        };
        let plan = compute_splits(&names, &cfg).unwrap();
        assert_eq!(plan.emit, vec![0, 1]);
        assert_eq!(plan.train, vec![1]);
        assert_eq!(plan.eval, vec![0]);
        assert!(plan.validation.is_empty());
    }

    #[test]
    fn filename_eval_split_local_train_all_selected() {
        let names = vec!["img_eval_b.png".into(), "img_train_a.png".into()];
        let cfg = SplitConfig {
            eval_mode: "filename".into(),
            split: "eval".into(),
            validation_fraction: 0.25, // forced to 0 for eval-only
            ..Default::default()
        };
        let plan = compute_splits(&names, &cfg).unwrap();
        assert_eq!(plan.emit, vec![0]);
        // DataManager eval-only: selected frames are all local train.
        assert_eq!(plan.train, vec![0]);
        assert!(plan.eval.is_empty());
        assert!(plan.validation.is_empty());
    }

    #[test]
    fn table_driven_split_mode_validation() {
        // 4 eval modes × 3 splits × validation {0, 0.25}.
        let names_filename = vec![
            "img_eval_b.png".into(),
            "img_train_a.png".into(),
            "img_train_c.png".into(),
            "img_train_d.png".into(),
        ];
        let names_plain: Vec<String> = (0..8).map(|i| format!("f{i}.png")).collect();
        let modes = ["all", "fraction", "interval", "filename"];
        let splits = ["all", "train", "eval"];
        let val_fracs = [0.0f32, 0.25];
        for mode in modes {
            for split in splits {
                for &validation_fraction in &val_fracs {
                    let names = if mode == "filename" {
                        names_filename.clone()
                    } else {
                        names_plain.clone()
                    };
                    let cfg = SplitConfig {
                        eval_mode: mode.into(),
                        train_split_fraction: 0.75,
                        eval_interval: 4,
                        split: split.into(),
                        validation_fraction,
                    };
                    let plan = compute_splits(&names, &cfg).unwrap();
                    let n_emitted = plan.emit.len();
                    for idx in plan
                        .train
                        .iter()
                        .chain(plan.eval.iter())
                        .chain(plan.validation.iter())
                    {
                        assert!(
                            *idx < n_emitted,
                            "mode={mode} split={split} val={validation_fraction}: index {idx} >= emitted {n_emitted}"
                        );
                    }
                    let train_set: std::collections::BTreeSet<_> =
                        plan.train.iter().copied().collect();
                    let val_set: std::collections::BTreeSet<_> =
                        plan.validation.iter().copied().collect();
                    assert!(
                        train_set.is_disjoint(&val_set),
                        "mode={mode} split={split} val={validation_fraction}"
                    );
                    if mode != "all" || split != "all" {
                        let eval_set: std::collections::BTreeSet<_> =
                            plan.eval.iter().copied().collect();
                        assert!(
                            train_set.is_disjoint(&eval_set),
                            "mode={mode} split={split}: train mixes eval"
                        );
                    }
                    if split == "eval" {
                        // Local train covers every selected frame; validation forced 0.
                        assert_eq!(
                            plan.train,
                            (0..n_emitted).collect::<Vec<_>>(),
                            "mode={mode} eval-only local train"
                        );
                        assert!(plan.eval.is_empty() && plan.validation.is_empty());
                    }
                    if split == "train" {
                        assert!(plan.eval.is_empty());
                        assert!(!plan.train.is_empty(), "empty train illegal");
                        if validation_fraction == 0.0 {
                            assert!(plan.validation.is_empty());
                        }
                    }
                    if split == "all" && mode == "filename" {
                        assert!(!plan.train.contains(&0), "eval frame wrongly in train");
                        assert!(plan.eval.contains(&0));
                    }
                }
            }
        }
    }

    #[test]
    fn eval_mode_all_coincidence_on_all_split() {
        let names: Vec<String> = (0..4).map(|i| format!("f{i}.png")).collect();
        let cfg = SplitConfig {
            eval_mode: "all".into(),
            split: "all".into(),
            validation_fraction: 0.25,
            ..Default::default()
        };
        let plan = compute_splits(&names, &cfg).unwrap();
        assert_eq!(plan.eval, vec![0, 1, 2, 3]);
        assert!(!plan.train.is_empty());
        assert!(plan.train.iter().all(|i| !plan.validation.contains(i)));
    }
}
