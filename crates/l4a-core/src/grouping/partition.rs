use super::candidate::BundleCandidate;
use super::clustering::has_conflicting_versions;
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(super) fn authors_compatible(a: &BundleCandidate, b: &BundleCandidate) -> bool {
    if let (Some(sa), Some(sb)) = (&a.steam_creator_id, &b.steam_creator_id) {
        return sa == sb;
    }
    if let (Some(na), Some(nb)) = (&a.author_name_key, &b.author_name_key) {
        return na == nb;
    }
    true
}

pub(super) fn group_authors_consistent(members: &[&BundleCandidate]) -> bool {
    let steam_ids: HashSet<&str> = members
        .iter()
        .filter_map(|m| m.steam_creator_id.as_deref())
        .collect();
    if steam_ids.len() > 1 {
        return false;
    }
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            if !authors_compatible(members[i], members[j]) {
                return false;
            }
        }
    }
    let non_steam_names: HashSet<&str> = members
        .iter()
        .filter(|m| m.steam_creator_id.is_none())
        .filter_map(|m| m.author_name_key.as_deref())
        .collect();
    non_steam_names.len() <= 1
}

pub(super) fn explicit_releases_compatible(a: &BundleCandidate, b: &BundleCandidate) -> bool {
    if let (Some(ta), Some(tb)) = (&a.title_release_key, &b.title_release_key) {
        if ta != tb {
            return false;
        }
    }
    if let (Some(fa), Some(fb)) = (&a.file_release_key, &b.file_release_key) {
        if fa != fb {
            return false;
        }
    }
    if let (Some(ea), Some(eb)) = (
        a.effective_release_key(false),
        b.effective_release_key(false),
    ) {
        if ea != eb {
            return false;
        }
    }
    true
}

pub(super) fn group_author_signature(members: &[&BundleCandidate]) -> String {
    if let Some(sid) = members.iter().find_map(|m| m.steam_creator_id.as_deref()) {
        return format!("sid:{sid}");
    }
    if let Some(name) = members.iter().find_map(|m| m.author_name_key.as_deref()) {
        return format!("author:{name}");
    }
    String::new()
}

pub(super) fn group_release_signature(members: &[&BundleCandidate]) -> String {
    if let Some(rel) = members.iter().find_map(|m| m.effective_release_key(false)) {
        return format!("rel:{rel}");
    }
    let versions: BTreeSet<&str> = members
        .iter()
        .filter_map(|m| m.version_key.as_deref())
        .collect();
    if versions.len() == 1 {
        if let Some(ver) = versions.into_iter().next() {
            return format!("ver:{ver}");
        }
    }
    String::new()
}

fn cluster_internally_valid(members: &[&BundleCandidate], prefer_file: bool) -> bool {
    group_authors_consistent(members) && group_releases_consistent(members, prefer_file)
}

pub(super) fn group_releases_consistent(members: &[&BundleCandidate], prefer_file: bool) -> bool {
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            let a = members[i];
            let b = members[j];
            if !explicit_releases_compatible(a, b) {
                return false;
            }
            if let (Some(ra), Some(rb)) = (
                a.effective_release_key(prefer_file),
                b.effective_release_key(prefer_file),
            ) {
                if ra != rb {
                    return false;
                }
            }
        }
    }
    true
}

fn effective_part_or_map_rank(cand: &BundleCandidate, prefer_file: bool) -> u32 {
    let fallback_rank = if !cand.addon.maps.is_empty() || cand.has_campaign_signal {
        0
    } else {
        500
    };
    cand.part_rank_for(prefer_file).unwrap_or(fallback_rank)
}

fn wildcard_collides_with_bucket(
    w: &BundleCandidate,
    bucket: &[&BundleCandidate],
    prefer_file: bool,
    check_zero_rank: bool,
) -> bool {
    if check_zero_rank {
        let r = effective_part_or_map_rank(w, prefer_file);
        if r < 100
            && bucket
                .iter()
                .any(|ex| effective_part_or_map_rank(ex, prefer_file) == r)
        {
            return true;
        }
    }
    if let Some(r) = w
        .part_rank_for(prefer_file)
        .filter(|&r| (1..100).contains(&r))
    {
        let colliding: Vec<&&BundleCandidate> = bucket
            .iter()
            .filter(|ex| ex.part_rank_for(prefer_file) == Some(r))
            .collect();
        if !colliding.is_empty() {
            let Some(wv) = w.version_key.as_deref() else {
                return true;
            };
            let distinct_versions = colliding
                .iter()
                .all(|ex| ex.version_key.as_deref().is_some_and(|ev| ev != wv));
            let has_same_version_sibling = bucket
                .iter()
                .any(|sib| sib.version_key.as_deref() == Some(wv));
            if !(distinct_versions && has_same_version_sibling) {
                return true;
            }
        }
    }
    false
}

fn wildcard_proposals_conflict(
    w1: &BundleCandidate,
    w2: &BundleCandidate,
    bucket: &[&BundleCandidate],
    prefer_file: bool,
    check_zero_rank: bool,
) -> bool {
    if !cluster_internally_valid(&[w1, w2], prefer_file) {
        return true;
    }
    let mut combined = Vec::with_capacity(bucket.len() + 2);
    combined.extend_from_slice(bucket);
    combined.push(w1);
    combined.push(w2);
    if !cluster_internally_valid(&combined, prefer_file) {
        return true;
    }
    if check_zero_rank {
        let r1 = effective_part_or_map_rank(w1, prefer_file);
        let r2 = effective_part_or_map_rank(w2, prefer_file);
        if r1 < 100 && r1 == r2 {
            return true;
        }
    }
    if let (Some(r1), Some(r2)) = (w1.part_rank_for(prefer_file), w2.part_rank_for(prefer_file)) {
        if (1..100).contains(&r1)
            && r1 == r2
            && !matches!((&w1.version_key, &w2.version_key), (Some(v1), Some(v2)) if v1 != v2)
        {
            return true;
        }
    }
    false
}

type WildcardScore = (bool, bool, bool, bool, bool, bool);

fn wildcard_bucket_score(
    w: &BundleCandidate,
    bucket: &[&BundleCandidate],
    prefer_file: bool,
) -> WildcardScore {
    let same_steam = w.steam_creator_id.as_deref().is_some_and(|sid| {
        bucket
            .iter()
            .any(|ex| ex.steam_creator_id.as_deref() == Some(sid))
    });
    let same_author_name = w.author_name_key.as_deref().is_some_and(|na| {
        bucket
            .iter()
            .any(|ex| ex.author_name_key.as_deref() == Some(na))
    });
    let same_release = w.effective_release_key(prefer_file).is_some_and(|release| {
        bucket
            .iter()
            .any(|ex| ex.effective_release_key(prefer_file) == Some(release))
    });
    let same_version = w.version_key.as_deref().is_some_and(|ver| {
        bucket
            .iter()
            .any(|ex| ex.version_key.as_deref() == Some(ver))
    });
    let w_rank = effective_part_or_map_rank(w, prefer_file);
    let no_role_rank_collision = !bucket
        .iter()
        .any(|ex| effective_part_or_map_rank(ex, prefer_file) == w_rank);
    let complementary_maps =
        w.has_campaign_signal || bucket.iter().any(|ex| ex.has_campaign_signal);
    (
        same_steam,
        same_author_name,
        same_release,
        same_version,
        no_role_rank_collision,
        complementary_maps,
    )
}

fn split_dimension<'a, F>(
    candidates: Vec<&'a BundleCandidate>,
    key_fn: F,
) -> (
    BTreeMap<&'a str, Vec<&'a BundleCandidate>>,
    Vec<&'a BundleCandidate>,
)
where
    F: Fn(&'a BundleCandidate) -> Option<&'a str>,
{
    let mut concrete_map: BTreeMap<&'a str, Vec<&'a BundleCandidate>> = BTreeMap::new();
    let mut wildcards: Vec<&'a BundleCandidate> = Vec::new();
    for cand in candidates {
        if let Some(k) = key_fn(cand) {
            concrete_map.entry(k).or_default().push(cand);
        } else {
            wildcards.push(cand);
        }
    }
    (concrete_map, wildcards)
}

/// 同时评估缺少分组键的候选项，归属由已有成员决定。
fn assign_wildcards<'a>(
    base_buckets: Vec<Vec<&'a BundleCandidate>>,
    wildcards: Vec<&'a BundleCandidate>,
    prefer_file: bool,
    check_zero_rank: bool,
) -> Vec<Vec<&'a BundleCandidate>> {
    if base_buckets.is_empty() {
        return if wildcards.is_empty() {
            Vec::new()
        } else {
            vec![wildcards]
        };
    }

    let mut proposals: Vec<Vec<(&'a BundleCandidate, WildcardScore)>> =
        vec![Vec::new(); base_buckets.len()];
    let mut residual: Vec<&'a BundleCandidate> = Vec::new();

    for w in wildcards {
        let mut eligible: Vec<(usize, WildcardScore)> = base_buckets
            .iter()
            .enumerate()
            .filter(|(_, bucket)| {
                let mut probe = Vec::with_capacity(bucket.len() + 1);
                probe.extend_from_slice(bucket);
                probe.push(w);
                cluster_internally_valid(&probe, prefer_file)
                    && !wildcard_collides_with_bucket(w, bucket, prefer_file, check_zero_rank)
            })
            .map(|(idx, bucket)| (idx, wildcard_bucket_score(w, bucket, prefer_file)))
            .collect();

        if eligible.is_empty() {
            residual.push(w);
        } else if eligible.len() == 1 {
            let (idx, score) = eligible[0];
            proposals[idx].push((w, score));
        } else {
            eligible.sort_by_key(|entry| std::cmp::Reverse(entry.1));
            if eligible[0].1 > eligible[1].1 {
                let (idx, score) = eligible[0];
                proposals[idx].push((w, score));
            } else {
                residual.push(w);
            }
        }
    }

    let mut out: Vec<Vec<&'a BundleCandidate>> = Vec::with_capacity(base_buckets.len() + 1);
    for (idx, mut bucket) in base_buckets.into_iter().enumerate() {
        let bucket_proposals = &proposals[idx];
        let mut accepted: Vec<&'a BundleCandidate> = Vec::new();
        for (i, &(w, score_i)) in bucket_proposals.iter().enumerate() {
            let has_blocking_peer =
                bucket_proposals
                    .iter()
                    .enumerate()
                    .any(|(j, &(peer, score_j))| {
                        i != j
                            && score_j >= score_i
                            && wildcard_proposals_conflict(
                                w,
                                peer,
                                &bucket,
                                prefer_file,
                                check_zero_rank,
                            )
                    });
            if has_blocking_peer {
                residual.push(w);
            } else {
                accepted.push(w);
            }
        }

        if !accepted.is_empty() {
            let mut combined = Vec::with_capacity(bucket.len() + accepted.len());
            combined.extend_from_slice(&bucket);
            combined.extend_from_slice(&accepted);
            if cluster_internally_valid(&combined, prefer_file) {
                bucket = combined;
            } else {
                residual.extend(accepted);
            }
        }
        out.push(bucket);
    }

    if !residual.is_empty() {
        out.push(residual);
    }
    out
}

fn partition_dimension<'a, F>(
    candidates: Vec<&'a BundleCandidate>,
    prefer_file: bool,
    check_zero_rank: bool,
    key_fn: F,
) -> Vec<Vec<&'a BundleCandidate>>
where
    F: Fn(&'a BundleCandidate) -> Option<&'a str>,
{
    let (concrete, wildcards) = split_dimension(candidates, key_fn);
    assign_wildcards(
        concrete.into_values().collect(),
        wildcards,
        prefer_file,
        check_zero_rank,
    )
}

fn partition_by_release(
    candidates: Vec<&BundleCandidate>,
    prefer_file: bool,
) -> Vec<Vec<&BundleCandidate>> {
    partition_dimension(candidates, prefer_file, false, |c| {
        c.title_release_key.as_deref()
    })
    .into_iter()
    .flat_map(|cluster| {
        partition_dimension(cluster, prefer_file, false, |c| {
            c.file_release_key.as_deref()
        })
    })
    .flat_map(|cluster| {
        partition_dimension(cluster, prefer_file, false, |c| {
            c.effective_release_key(prefer_file)
        })
    })
    .collect()
}

pub(super) fn partition_by_author_and_release(
    candidates: Vec<&BundleCandidate>,
    prefer_file: bool,
) -> Vec<Vec<&BundleCandidate>> {
    // 先拆分同一 Steam 创作者的各个发布版，再评估缺少 Steam ID 的本地分包归属。
    let (creators, local_parts) = split_dimension(candidates, |c| c.steam_creator_id.as_deref());
    let steam_releases = creators
        .into_values()
        .flat_map(|cluster| partition_by_release(cluster, prefer_file))
        .collect();
    let mut out = Vec::new();
    for cluster in assign_wildcards(steam_releases, local_parts, prefer_file, false) {
        if cluster.iter().any(|c| c.steam_creator_id.is_some()) {
            out.push(cluster);
        } else {
            let authors = partition_dimension(cluster, prefer_file, false, |c| {
                c.author_name_key.as_deref()
            });
            out.extend(
                authors
                    .into_iter()
                    .flat_map(|members| partition_by_release(members, prefer_file)),
            );
        }
    }
    out
}

/// 当同一候选组内存在重复分包序号（如两个 Part 1）时，按 `addonversion` 正交切分为独立的发布包。
pub(super) fn split_duplicate_parts_across_versions(
    members: Vec<&BundleCandidate>,
    prefer_file: bool,
) -> Vec<Vec<&BundleCandidate>> {
    if !has_duplicate_part_ranks(&members, prefer_file) {
        return vec![members];
    }
    partition_dimension(members, prefer_file, true, |c| c.version_key.as_deref())
}

fn has_duplicate_part_ranks(members: &[&BundleCandidate], prefer_file: bool) -> bool {
    let mut seen = HashSet::new();
    for m in members {
        let rank = effective_part_or_map_rank(m, prefer_file);
        if rank < 100 && !seen.insert(rank) {
            return true;
        }
    }
    false
}

pub(super) fn has_conflicting_duplicate_parts(
    members: &[&BundleCandidate],
    prefer_file: bool,
) -> bool {
    if has_conflicting_versions(members) && has_duplicate_part_ranks(members, prefer_file) {
        return true;
    }
    // 同一整合包内绝不允许出现两个相同显式序号的分包（如同时包含两个 `Part 1`）
    let mut numbered_seen = HashSet::new();
    for m in members {
        if let Some(rank) = m.part_rank_for(prefer_file) {
            if (1..100).contains(&rank) && !numbered_seen.insert(rank) {
                return true;
            }
        }
    }
    false
}
