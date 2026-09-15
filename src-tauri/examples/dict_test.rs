// 词典纠错 headless 测试: cargo run --bin dict_test
use denpa_jack_lib::dict::TextCorrector;
use denpa_jack_lib::settings::DictEntry;

fn main() {
    let entries = vec![
        DictEntry { term: "谢克数学".into(), variants: vec!["些克数学".into(), "歇课数学".into()], guard_words: vec![], boost: 3 },
        DictEntry { term: "腾讯云".into(), variants: vec![], guard_words: vec![], boost: 2 },
        DictEntry { term: "机器学习".into(), variants: vec![], guard_words: vec![], boost: 2 },
    ];
    let corr = TextCorrector { entries, normalizations: vec![] };

    let cases: Vec<(&str, &str, &str)> = vec![
        ("歇课数学很厉害", "谢克数学很厉害", "变体替换"),
        ("今天讲机器学西", "今天讲机器学习", "拼音模糊 0.5"),
        ("我们一起学习吧", "我们一起学习吧", "C1 护栏: 不该替换"),
        ("腾讯云上部署", "腾讯云上部署", "已含正确写法跳过"),
        ("机器学倁真难", "机器学倁真难", "C1: 1.0 新音节不放行"),
    ];
    for (input, want, note) in cases {
        let (out, m) = corr.correct(input);
        let ok = out == want;
        println!("{} [{}] {} -> {} (kind={:?})", if ok { "PASS" } else { "FAIL" }, note, input, out, m.iter().map(|x| x.kind.as_str()).collect::<Vec<_>>());
    }
}
