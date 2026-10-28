//! Train / eval / validation splits (`DatasetCommon.cpp:183-258`).

use std::path::Path;

use spirula_core::ErrorCode;

use super::budget::err;

/// NumPy-style `linspace(0, n-1, count, dtype=int)` (truncate toward zero).
pub fn linspace_indices(n: i64, count: i64) -> Vec<i64> {
    let mut out = Vec::new();
    if count <= 0 || n <= 0 {
        return out;
    }
    out.reserve(count as usize);
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
    /// `"train"` | `"eval"` | `"all"` — `"all"` returns every index (Rust extension for summaries).
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

/// Train keep-set, then optionally return eval complement (`train_subset`).
pub fn train_subset(
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

    match cfg.split.as_str() {
        "eval" => {
            if cfg.eval_mode == "all" {
                return Ok(keep);
            }
            let mut is_train = vec![false; n as usize];
            for i in &keep {
                is_train[*i as usize] = true;
            }
            let mut other = Vec::new();
            for i in 0..n {
                if !is_train[i as usize] {
                    other.push(i);
                }
            }
            // Empty eval is legal.
            Ok(other)
        }
        "train" => Ok(keep),
        "all" => Ok((0..n).collect()),
        other => Err(err(
            ErrorCode::InvalidArgument,
            "split",
            format!("unknown split '{other}' (expected 'train', 'eval', or 'all')"),
        )),
    }
}

/// Validation hold-out over the current subset (`assign_val_split`).
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

type SplitTriple = (Vec<usize>, Vec<usize>, Vec<usize>);

/// Compute train + eval index lists for a sorted name list, then validation on the selected split.
pub fn compute_splits(
    names: &[String],
    cfg: &SplitConfig,
) -> Result<SplitTriple, spirula_core::DomainError> {
    let n = names.len() as i64;
    let selected = train_subset(n, names, cfg)?;
    // Also compute the complementary eval set for summaries (when split=train).
    let eval_cfg = SplitConfig {
        split: "eval".into(),
        ..cfg.clone()
    };
    let eval_idx = train_subset(n, names, &eval_cfg)?;
    let train_all_cfg = SplitConfig {
        split: "train".into(),
        ..cfg.clone()
    };
    let train_idx = train_subset(n, names, &train_all_cfg)?;

    let selected_usize: Vec<usize> = selected.iter().map(|i| *i as usize).collect();
    let val_frac = if cfg.split == "eval" {
        0.0
    } else {
        cfg.validation_fraction
    };
    let (train_local, val_local) = assign_val_split(selected_usize.len() as i64, val_frac)?;
    // Map local subset indices back to global frame indices.
    let train_global: Vec<usize> = train_local.iter().map(|i| selected_usize[*i]).collect();
    let val_global: Vec<usize> = val_local.iter().map(|i| selected_usize[*i]).collect();
    let eval_global: Vec<usize> = eval_idx.iter().map(|i| *i as usize).collect();
    let _ = train_idx; // train_global already reflects selected split
    Ok((train_global, eval_global, val_global))
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
        // all names have neither train nor eval → error before empty-train
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
}
