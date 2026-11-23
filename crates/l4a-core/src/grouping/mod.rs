//! 按四层清晰模型管理：
//! 1. `Bundle`: 战役/地图多包物理整合（支持 `locked` 保护与跨分包地图/封面聚合）
//! 2. `TrackedCollection`: 上游工坊合集追踪
//! 3. `UserGroup`: 多对多自定义分类/标签
//! 4. `LoadoutProfile`: 一键启用预设

mod auto_bundles;
mod candidate;
mod clustering;
mod manage;
mod parsing;
mod partition;

#[cfg(test)]
mod tests;

pub use auto_bundles::{
    create_manual_bundle, delete_bundle, rebuild_auto_bundles, set_bundle_locked,
};
pub use manage::{
    apply_profile, delete_profile, delete_user_group, save_profile, save_user_group,
    track_workshop_collection, untrack_workshop_collection,
};
