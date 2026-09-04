// 词典纠错(Rust 移植自 Swift TextCorrector)
// 管道: 正则规范化 → 变体精确替换(防自替换) → 拼音窗口模糊(C1: 仅 3+ 字, 允许 0.5 一个近音音节)
use pinyin::ToPinyin;

pub const NEAR_GROUPS: [&str; 10] = [
    "zhz", "chc", "shs", "nl", "fh", "anang", "eneng", "ining", "ianianguang", // 逐组拆分处理
    "uanuang",
];

fn is_cjk(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// 汉字序列 → 无声调拼音音节数组(非汉字跳过)
pub fn pinyin_syllables(text: &str) -> Vec<String> {
    text.to_pinyin()
        .filter_map(|py| py.map(|p| p.plain().to_string()))
        .collect()
}

fn near_equal(a: &str, b: &str) -> bool {
    for g in [
        ["zh", "z"],
        ["ch", "c"],
        ["sh", "s"],
        ["n", "l"],
        ["f", "h"],
        ["an", "ang"],
        ["en", "eng"],
        ["in", "ing"],
        ["ian", "iang"],
        ["uan", "uang"],
    ] {
        let ga: Vec<&str> = g.iter().copied().filter(|x| a.ends_with(x)).collect();
        let gb: Vec<&str> = g.iter().copied().filter(|x| b.ends_with(x)).collect();
        if let (Some(x), Some(y)) = (ga.first(), gb.first()) {
            if a[..a.len() - x.len()] == b[..b.len() - y.len()] {
                return true;
            }
        }
        for x in g {
            for y in g {
                if x != y && a.starts_with(x) && b.starts_with(y) && a[x.len()..] == b[y.len()..] {
                    return true;
                }
            }
        }
    }
    false
}

fn syllable_dist(a: &str, b: &str) -> f64 {
    if a == b {
        0.0
    } else if near_equal(a, b) {
        0.5
    } else {
        1.0
    }
}

fn seq_dist(sa: &[String], sb: &[String]) -> f64 {
    let m = sa.len();
    let n = sb.len();
    let mut dp = vec![vec![0.0f64; n + 1]; m + 1];
    for i in 0..=m {
        dp[i][0] = i as f64;
    }
    for j in 0..=n {
        dp[0][j] = j as f64;
    }
    for i in 1..=m {
        for j in 1..=n {
            let sub = dp[i - 1][j - 1] + syllable_dist(&sa[i - 1], &sb[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1.0).min(dp[i][j - 1] + 1.0).min(sub);
        }
    }
    dp[m][n]
}

pub struct TextCorrector {
    pub entries: Vec<crate::settings::DictEntry>,
    pub normalizations: Vec<(String, String)>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Match {
    pub term: String,
    pub kind: String, // variant | pinyin
    pub score: f64,
}

impl TextCorrector {
    pub fn correct(&self, input: &str) -> (String, Vec<Match>) {
        let mut text = input.to_string();
        // 1) 正则规范化(按 pattern 长度降序)
        let mut norms = self.normalizations.clone();
        norms.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        for (p, r) in norms {
            if let Ok(re) = regex::Regex::new(&p) {
                text = re.replace_all(&text, r.as_str()).to_string();
            }
        }
        let mut matches = vec![];
        let mut entries = self.entries.clone();
        entries.sort_by(|a, b| b.term.chars().count().cmp(&a.term.chars().count()));
        for e in &entries {
            if text.contains(&e.term) {
                continue;
            }
            if e.guard_words.iter().any(|g| text.contains(g)) {
                continue;
            }
            // 2) 显式变体精确替换(最长优先; 跳过是词条子串的变体, 防自替换)
            let mut did_replace = false;
            let mut forms: Vec<&String> = std::iter::once(&e.term).chain(e.variants.iter()).collect();
            forms.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
            for v in forms {
                if v == &e.term || e.term.contains(v.as_str()) {
                    continue;
                }
                while let Some(pos) = text.find(v.as_str()) {
                    text.replace_range(pos..pos + v.len(), &e.term);
                    did_replace = true;
                    matches.push(Match { term: e.term.clone(), kind: "variant".into(), score: 0.0 });
                }
            }
            if did_replace || text.contains(&e.term) {
                continue;
            }
            // 3) 拼音窗口模糊(C1: 仅 3+ 字; C2: 近音 0.5; 新音节 1.0 不放行)
            let term_py = pinyin_syllables(&e.term);
            if term_py.len() < 3 {
                continue;
            }
            let allowed = 0.5f64;
            let chars: Vec<char> = text.chars().collect();
            if term_py.len() > chars.len() {
                continue;
            }
            let mut best: Option<(usize, f64)> = None;
            for i in 0..=(chars.len() - term_py.len()) {
                let window: String = chars[i..i + term_py.len()].iter().collect();
                if !window.chars().all(is_cjk) {
                    continue;
                }
                let wp = pinyin_syllables(&window);
                if wp.len() != term_py.len() {
                    continue;
                }
                let d = seq_dist(&term_py, &wp);
                if d <= allowed && d < best.map(|b| b.1).unwrap_or(f64::INFINITY) {
                    best = Some((i, d));
                }
            }
            if let Some((idx, score)) = best {
                let start_byte = chars[..idx].iter().map(|c| c.len_utf8()).sum::<usize>();
                let end_byte = start_byte
                    + chars[idx..idx + term_py.len()].iter().map(|c| c.len_utf8()).sum::<usize>();
                text.replace_range(start_byte..end_byte, &e.term);
                matches.push(Match { term: e.term.clone(), kind: "pinyin".into(), score });
            }
        }
        (text, matches)
    }
}
