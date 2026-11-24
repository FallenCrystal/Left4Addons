use super::auth::{ActiveFlowHandle, SteamLoginConfirmation};
use super::mapping::{details_to_record, map_detail};
use super::*;
use crate::config::CatalogConfigStore;
use l4a_protocol::{SteamAuthFlowKind, SteamAuthFlowState};
use std::sync::Arc;
use steam_vent::auth::{
    AuthConfirmationHandler, ConfirmationAction, UserProvidedAuthConfirmationHandler,
};
use steam_vent::EResult;
use steam_vent_proto_common::protobuf::EnumOrUnknown;
use steam_vent_proto_steam::steammessages_publishedfile_steamclient::{
    CPublishedFile_QueryFiles_Request, PublishedFileDetails,
};

#[test]
fn successful_account_only_details_become_tombstones() {
    for visibility in [1, 2] {
        let details = PublishedFileDetails {
            publishedfileid: Some(42),
            result: Some(1),
            visibility: Some(visibility),
            title: Some("Account-visible title".into()),
            file_url: Some("https://example.test/private.vpk".into()),
            ..Default::default()
        };
        assert!(matches!(
            map_detail(&details, None, Utc::now()).map(|obs| obs.outcome),
            Some(WorkshopFetchOutcome::PrivateOrUnavailable {
                workshop_id: WorkshopId(42),
                ..
            })
        ));
    }
}

#[test]
fn transient_detail_errors_do_not_generate_tombstones() {
    for result in [EResult::Busy, EResult::Timeout, EResult::ServiceUnavailable] {
        let details = PublishedFileDetails {
            publishedfileid: Some(42),
            result: Some(result as u32),
            ..Default::default()
        };
        assert!(map_detail(&details, None, Utc::now()).is_none());
    }
}

#[test]
fn unlisted_details_are_available_by_id() {
    let details = PublishedFileDetails {
        publishedfileid: Some(42),
        result: Some(EResult::OK as u32),
        visibility: Some(3),
        ..Default::default()
    };
    let observation = map_detail(&details, None, Utc::now()).unwrap();
    assert!(!observation.listed);
    assert!(matches!(
        observation.outcome,
        WorkshopFetchOutcome::Found { .. }
    ));
}

#[tokio::test]
async fn app_only_confirmation_keeps_the_polling_action() {
    use steam_vent_proto_steam::steammessages_auth_steamclient::{
        CAuthentication_AllowedConfirmation, EAuthSessionGuardType,
    };
    let method = CAuthentication_AllowedConfirmation {
        confirmation_type: Some(EnumOrUnknown::new(
            EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceConfirmation,
        )),
        ..Default::default()
    };
    let confirmation = SteamLoginConfirmation(UserProvidedAuthConfirmationHandler::new(
        tokio::io::empty(),
        tokio::io::sink(),
    ));
    assert!(matches!(
        confirmation.handle_confirmation(&[method.into()]).await,
        Some(ConfirmationAction::None)
    ));
}

#[tokio::test]
async fn replacing_a_pending_flow_aborts_its_task_and_blocks_stale_updates() {
    let steam = SteamCmClient::new(Arc::new(CatalogConfigStore::new(
        None,
        crate::config::CatalogConfig::default(),
    )));
    let old = tokio::spawn(std::future::pending::<()>());
    let mut slot = steam.inner.auth_flow.lock().await;
    let previous = slot.begin();
    slot.active = Some(ActiveFlowHandle {
        status: SteamAuthFlowStatus {
            flow_id: "old-qr".into(),
            kind: SteamAuthFlowKind::Qr,
            state: SteamAuthFlowState::Authenticating,
            challenge_url: None,
            qr_svg: None,
            code_prompt: None,
            account_name: None,
            error: None,
            updated_at: Utc::now(),
        },
        abort_handle: Some(old.abort_handle()),
        code_writer: None,
    });
    let current = slot.begin();
    assert!(slot.ensure_current(current).is_ok());
    drop(slot);
    assert!(old.await.unwrap_err().is_cancelled());
    assert!(steam
        .inner
        .set_qr_challenge(previous, "https://example.test/old")
        .await
        .is_err());
}

#[test]
fn build_search_files_proto_omits_creator_appid_and_serializes_target() {
    use steam_vent_proto_common::protobuf::Message;

    let req = SteamSearchRequest {
        query_type: QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
        file_type: MATCHING_FILE_TYPE_ITEMS,
        page: 2,
        num_per_page: 24,
        search_text: Some("Early Days".to_string()),
        required_tags: vec!["Campaigns".to_string()],
        language: 6,
        days: Some(7),
        search_text_target: Some(SteamSearchTarget::Title),
    };
    let proto = build_search_files_proto(550, &req);
    assert_eq!(proto.appid, Some(550));
    assert!(proto.creator_appid.is_none());
    assert_eq!(proto.query_type, Some(QUERY_TYPE_RANKED_BY_TEXT_SEARCH));
    assert_eq!(proto.filetype, Some(MATCHING_FILE_TYPE_ITEMS));
    assert_eq!(proto.page, Some(2));
    assert_eq!(proto.numperpage, Some(24));
    assert_eq!(proto.search_text.as_deref(), Some("Early Days"));
    assert_eq!(proto.requiredtags, vec!["Campaigns".to_string()]);
    assert_eq!(proto.language, Some(6));
    assert_eq!(proto.days, Some(7));
    assert_eq!(proto.return_short_description, Some(false));
    assert_eq!(proto.return_details, Some(true));

    let bytes = proto.write_to_bytes().unwrap();
    let decoded = CPublishedFile_QueryFiles_Request::parse_from_bytes(&bytes).unwrap();
    assert_eq!(
        decoded
            .special_fields
            .unknown_fields()
            .get(PROTO_FIELD_SEARCH_TEXT_TARGET)
            .and_then(|v| match v {
                steam_vent_proto_common::protobuf::UnknownValueRef::Varint(val) => Some(val),
                _ => None,
            }),
        Some(SteamSearchTarget::Title as u64)
    );

    let default_target_proto = build_search_files_proto(
        550,
        &SteamSearchRequest {
            search_text_target: Some(SteamSearchTarget::All),
            ..req
        },
    );
    assert!(default_target_proto
        .special_fields
        .unknown_fields()
        .get(PROTO_FIELD_SEARCH_TEXT_TARGET)
        .is_none());
}

#[test]
fn details_to_record_prefers_canonical_tags_over_localized_display_names() {
    use steam_vent_proto_steam::steammessages_publishedfile_steamclient::published_file_details::Tag;

    let details = PublishedFileDetails {
        publishedfileid: Some(9001),
        title: Some("Localized Map".into()),
        tags: vec![
            Tag {
                tag: Some("Campaigns".into()),
                display_name: Some("战役".into()),
                ..Default::default()
            },
            Tag {
                tag: Some("   ".into()),
                display_name: Some("Co-op".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let record = details_to_record(WorkshopId(9001), &details, None, Utc::now());
    assert_eq!(record.tags, vec!["Campaigns", "Co-op"]);
}
