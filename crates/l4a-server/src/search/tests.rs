use super::pinyin::pinyin_fts_clause;
use super::query::fts_quote;
use super::*;

#[test]
fn compact_and_camel_case_matching_finds_daybreak_and_earlydays() {
    let day_break_doc = build_indexed_document("Day Break v2", Some("Hoangzp"), None);
    assert!(day_break_doc.title_compact.contains("daybreak"));

    for query_str in ["DayBreak", "daybreak", "Day Break", "day_break", "daybre"] {
        let q = parse_query(query_str).unwrap();
        let rank = score_primary_candidate(&q, 2750097524, "Day Break v2", Some("Hoangzp"));
        assert!(rank.is_some(), "expected {query_str} to match Day Break v2");
        assert!(
            rank.unwrap().tier <= 1,
            "expected top tier for {query_str}, got {rank:?}"
        );
    }

    // Must NOT match mid-word boundary like "Friday Breaking"
    let q = parse_query("daybreak").unwrap();
    assert!(score_primary_candidate(&q, 1, "Friday Breaking", None).is_none());

    // EarlyDays matches ALL parts via title without relying on description
    let q = parse_query("EarlyDays").unwrap();
    for part in 1..=6 {
        let title = format!("Early Days PART {part}/6");
        let rank = score_primary_candidate(&q, 100 + part, &title, Some("perfect_buddy"));
        assert_eq!(
            rank,
            Some(MatchRank {
                tier: 1,
                sub_rank: 1
            })
        );
    }
}

#[test]
fn chinese_pinyin_matches_guangzhou_and_yichang_without_english_false_positives() {
    let gz_title = "广州增城 （Zengcheng）Lv8.06";
    let gz_doc = build_indexed_document(gz_title, Some("长远的梦"), None);
    for prefix in ["guangzho", "zengchen"] {
        assert!(gz_doc
            .title_pinyin
            .split_whitespace()
            .any(|key| key == prefix));
    }

    for q_str in [
        "guangzhou",
        "guang zhou",
        "guangzhouzengcheng",
        "广州",
        "增城",
        "zengcheng",
    ] {
        let q = parse_query(q_str).unwrap();
        let rank = score_primary_candidate(&q, 2396847377, gz_title, Some("长远的梦"));
        assert!(rank.is_some(), "expected {q_str} to match {gz_title}");
    }

    // 宜昌市V5.01 has no English in title but matches via pinyin
    let yc_q = parse_query("yichang").unwrap();
    assert_eq!(
        score_primary_candidate(&yc_q, 2802325803, "宜昌市V5.01", Some("JohNny.R")),
        Some(MatchRank {
            tier: 4,
            sub_rank: 0
        })
    );

    // Polyphone city names & ü -> v spelling
    for (q_str, title) in [
        ("chongqing", "重庆洪崖洞"),
        ("xiamen", "厦门鼓浪屿"),
        ("chengdu", "成都春熙路"),
        ("nvwu", "月光女巫替换"),
        ("yinhanghangzhang", "银行行长"),
    ] {
        let q = parse_query(q_str).unwrap();
        let doc = build_indexed_document(title, None, None);
        let clause = pinyin_fts_clause(q_str).unwrap();
        assert!(
            doc.title_pinyin
                .split_whitespace()
                .any(|key| clause.contains(&fts_quote(key))),
            "expected pinyin candidate for {q_str} in {title}: {}",
            doc.title_pinyin
        );
        assert!(score_primary_candidate(&q, 1, title, None).is_some());
    }

    // Single-syllable English words must NOT match single Chinese characters via pinyin
    for (q_str, title) in [("man", "满血复活"), ("gun", "铁棍武器"), ("die", "蝴蝶刀")] {
        let q = parse_query(q_str).unwrap();
        assert!(
            score_primary_candidate(&q, 1, title, None).is_none(),
            "{q_str} should not false-match {title}"
        );
    }
}

#[test]
fn literal_underscore_suffix_rejects_non_matching_word() {
    let q = parse_query("mission_").unwrap();
    assert!(score_primary_candidate(&q, 9001, "MISSION_", None).is_some());
    assert!(score_primary_candidate(&q, 9002, "mission_ Zulu", None).is_some());
    assert!(score_primary_candidate(&q, 9004, "An earlier Mission_ text", None).is_some());
    assert!(score_primary_candidate(&q, 9007, "MissionX", None).is_none());
}

#[test]
fn bbcode_stripping_removes_urls_and_img_tags_while_keeping_anchor_text() {
    let raw = "[img]https://imgur.com/earlydays.png[/img] [b]Check[/b] [url=https://steamcommunity.com/sharedfiles/filedetails/?id=123]Early Days[/url] https://example.com/daybreak";
    let cleaned = strip_bbcode_and_urls(raw);
    assert!(!cleaned.contains("imgur"));
    assert!(!cleaned.contains("steamcommunity"));
    assert!(!cleaned.contains("example.com"));
    assert!(cleaned.contains("Check"));
    assert!(cleaned.contains("Early Days"));
}

#[test]
fn steam_workshop_url_extracts_exact_id() {
    let q = parse_query(
        "https://steamcommunity.com/sharedfiles/filedetails/?id=2396847377&searchtext=foo",
    )
    .unwrap();
    assert_eq!(q.exact_workshop_id, Some(2396847377));
    assert!(q.terms.is_empty());
    assert_eq!(
        score_primary_candidate(&q, 2396847377, "广州增城", None),
        Some(MatchRank {
            tier: 0,
            sub_rank: 0
        })
    );
    assert!(score_primary_candidate(&q, 1, "foo", None).is_none());
}

#[test]
fn can_match_pinyin_precomputation_matches_query_shapes() {
    for non_pinyin in ["广州", "ab", "12345", "mission_", "a b", "!!!"] {
        let q = parse_query(non_pinyin).unwrap();
        assert!(
            !q.can_match_pinyin,
            "expected can_match_pinyin=false for {non_pinyin:?}"
        );
    }
    for pinyin_capable in [
        "guangzhou",
        "guang zhou",
        "guangzhou 8.06",
        "广州 zengcheng",
    ] {
        let q = parse_query(pinyin_capable).unwrap();
        assert!(
            q.can_match_pinyin,
            "expected can_match_pinyin=true for {pinyin_capable:?}"
        );
    }
}
