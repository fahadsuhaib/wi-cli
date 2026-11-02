//! Fuzzy file-name matching for the finder: case-smart subsequence matching, scored so that
//! consecutive characters, word and path-separator boundaries and matches in the file's own name
//! rank higher. Pure and client-side; the candidate list comes from one bounded command.

const MATCH: i32 = 16;
const CONSECUTIVE: i32 = 12;
const BOUNDARY_PATH: i32 = 14;
const BOUNDARY_WORD: i32 = 10;
const BOUNDARY_CAMEL: i32 = 8;
const IN_BASENAME: i32 = 4;
const GAP_OPEN: i32 = 3;
const GAP_EXT: i32 = 1;
const NEG: i32 = i32::MIN / 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Index into the candidate list.
    pub idx: usize,
    pub score: i32,
}

fn fold(c: char, sens: bool) -> char {
    if sens { c } else { c.to_lowercase().next().unwrap_or(c) }
}

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

fn bonus(cand: &[char], j: usize, base_start: usize) -> i32 {
    let mut b = if j >= base_start { IN_BASENAME } else { 0 };
    b += match j.checked_sub(1).map(|p| cand[p]) {
        None => BOUNDARY_PATH,
        Some(p) if is_sep(p) => BOUNDARY_PATH,
        Some(p) if "_-. ".contains(p) => BOUNDARY_WORD,
        Some(p) if p.is_lowercase() && cand[j].is_uppercase() => BOUNDARY_CAMEL,
        _ => 0,
    };
    b
}

/// Score `query` against `cand`, with the matched character positions. None if `query` is not a
/// subsequence. Spaces in the query are ignored; case matters only when the query has capitals.
pub fn score(query: &str, cand: &str) -> Option<(i32, Vec<usize>)> {
    let sens = super::smart_sensitive(query);
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(|c| fold(c, sens)).collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let raw: Vec<char> = cand.chars().collect();
    let folded: Vec<char> = raw.iter().map(|&c| fold(c, sens)).collect();
    let (m, n) = (q.len(), raw.len());
    if m > n {
        return None;
    }
    // Quick reject: is it a subsequence at all?
    let mut qi = 0;
    for &c in &folded {
        if qi < m && c == q[qi] {
            qi += 1;
        }
    }
    if qi < m {
        return None;
    }
    let base_start = raw.iter().rposition(|&c| is_sep(c)).map_or(0, |p| p + 1);
    // best[i][j]: best score with q[i] matched at cand[j]; from[i][j]: where q[i-1] was matched.
    let mut best = vec![vec![NEG; n]; m];
    let mut from = vec![vec![0usize; n]; m];
    for i in 0..m {
        let mut run = NEG; // best of best[i-1][k] minus gap, for k <= j-2
        let mut run_k = 0usize;
        for j in 0..n {
            if i > 0 && j >= 2 {
                let cand_run = best[i - 1][j - 2];
                let ext = if run > NEG { run - GAP_EXT } else { NEG };
                let open = if cand_run > NEG { cand_run - GAP_OPEN } else { NEG };
                // `run` covers k <= j-3 extended by one; `open` is k = j-2 (one skipped char).
                if open >= ext {
                    run = open;
                    run_k = j - 2;
                } else {
                    run = ext;
                }
            }
            if folded[j] != q[i] {
                continue;
            }
            let here = MATCH + bonus(&raw, j, base_start);
            if i == 0 {
                best[0][j] = here;
                continue;
            }
            let mut b = NEG;
            let mut k = 0;
            if j >= 1 && best[i - 1][j - 1] > NEG {
                b = best[i - 1][j - 1] + CONSECUTIVE;
                k = j - 1;
            }
            if run > b {
                b = run;
                k = run_k;
            }
            if b > NEG {
                best[i][j] = b + here;
                from[i][j] = k;
            }
        }
    }
    let (mut j, mut top) = (0, NEG);
    for (jj, &s) in best[m - 1].iter().enumerate() {
        // Ties go to the later end, which keeps matches near the end of the path (the file name).
        if s >= top {
            top = s;
            j = jj;
        }
    }
    if top <= NEG {
        return None;
    }
    let mut pos = vec![0; m];
    for i in (0..m).rev() {
        pos[i] = j;
        if i > 0 {
            j = from[i][j];
        }
    }
    // Shorter paths win ties.
    Some((top - (n as i32) / 2, pos))
}

/// Rank candidates for `query`, best first (ties keep list order). Empty query keeps everything.
pub fn rank(query: &str, cands: &[String]) -> Vec<Hit> {
    if query.chars().all(char::is_whitespace) {
        return (0..cands.len()).map(|idx| Hit { idx, score: 0 }).collect();
    }
    let mut hits: Vec<Hit> = cands.iter().enumerate().filter_map(|(idx, c)| score(query, c).map(|(score, _)| Hit { idx, score })).collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.idx.cmp(&b.idx)));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str, files: &[&str]) -> Vec<String> {
        let c: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        rank(query, &c).iter().map(|h| c[h.idx].clone()).collect()
    }

    const FILES: &[&str] = &["Cargo.toml", "README.md", "src/app.rs", "src/exec.rs", "src/main.rs", "src/search/mod.rs", "tests/cli.rs", "docs/notes/readme.txt"];

    #[test]
    fn subsequence_required() {
        assert!(score("xyz", "src/main.rs").is_none());
        assert!(score("mnrs", "src/main.rs").is_some());
        assert!(score("main.rs.x", "main.rs").is_none());
        assert!(score("", "anything").is_some());
    }

    #[test]
    fn mnrs_finds_main_rs_first() {
        let r = top("mnrs", FILES);
        assert_eq!(r[0], "src/main.rs");
    }

    #[test]
    fn positions_point_at_the_matched_characters() {
        let (_, p) = score("mnrs", "src/main.rs").unwrap();
        let chars: Vec<char> = "src/main.rs".chars().collect();
        assert_eq!(p.iter().map(|&i| chars[i]).collect::<String>(), "mnrs");
        assert!(p.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(p[0], 4, "the m of main, not an m earlier");
    }

    #[test]
    fn consecutive_beats_scattered() {
        let r = top("app", &["a_p_p.txt", "src/app.rs", "apple/x.txt"]);
        assert_eq!(r[0], "src/app.rs", "{r:?}");
    }

    #[test]
    fn basename_matches_beat_directory_matches() {
        let r = top("exec", &["exec/other.rs", "src/exec.rs", "e/x/e/c.rs"]);
        assert_eq!(r[0], "src/exec.rs");
        assert_eq!(top("mod", &["models/readme.md", "src/search/mod.rs"])[0], "src/search/mod.rs");
    }

    #[test]
    fn boundaries_score_higher() {
        let r = top("sm", &["xxsxxm.txt", "some_more.txt"]);
        assert_eq!(r[0], "some_more.txt");
        let r = top("fb", &["fooBar.ts", "foobar.ts"]);
        assert_eq!(r[0], "fooBar.ts", "camel case boundary");
    }

    #[test]
    fn smart_case() {
        assert_eq!(top("readme", FILES).len(), 2, "lowercase query ignores case");
        assert_eq!(top("README", FILES), vec!["README.md"], "a capital makes it exact");
    }

    #[test]
    fn shorter_paths_win_ties_and_order_is_stable() {
        let r = top("a", &["b/a", "a", "c/c/a"]);
        assert_eq!(r[0], "a");
        let same = top("x", &["x1", "x2", "x3"]);
        assert_eq!(same, vec!["x1", "x2", "x3"]);
    }

    #[test]
    fn empty_query_keeps_the_list() {
        assert_eq!(top("  ", FILES).len(), FILES.len());
        assert_eq!(top("", FILES)[0], FILES[0]);
    }

    #[test]
    fn spaces_in_the_query_are_ignored() {
        assert_eq!(top("src main", FILES)[0], "src/main.rs");
    }

    #[test]
    fn unicode_and_long_paths_do_not_panic() {
        assert!(score("日本", "docs/日本語/メモ.txt").is_some());
        let long = "a/".repeat(500) + "target.rs";
        assert!(score("trs", &long).is_some());
    }
}
