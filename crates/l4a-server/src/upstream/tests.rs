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

#[test]
fn map_detail_normalizes_zero_consumer_appid_and_blank_file_description() {
    let stub = PublishedFileDetails {
        publishedfileid: Some(9002),
        result: Some(9),
        consumer_appid: Some(0),
        ..Default::default()
    };
    let obs = map_detail(&stub, None, Utc::now()).expect("FileNotFound should map to Removed");
    assert_eq!(obs.consumer_appid, None);
    assert!(matches!(obs.outcome, WorkshopFetchOutcome::Removed { .. }));

    let active = PublishedFileDetails {
        publishedfileid: Some(9003),
        result: Some(1),
        visibility: Some(0),
        consumer_appid: Some(550),
        title: Some("Short Desc Map".into()),
        file_description: Some("   ".into()),
        short_description: Some("Fallback summary".into()),
        ..Default::default()
    };
    let obs = map_detail(&active, None, Utc::now()).unwrap();
    assert_eq!(obs.consumer_appid, Some(550));
    match obs.outcome {
        WorkshopFetchOutcome::Found { item } => {
            assert_eq!(item.description.as_deref(), Some("Fallback summary"));
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

fn wire_varint(out: &mut Vec<u8>, field_number: u32, value: u64) {
    use steam_vent_proto_common::protobuf::CodedOutputStream;
    let mut os = CodedOutputStream::vec(out);
    os.write_raw_varint32(field_number << 3).unwrap();
    os.write_raw_varint64(value).unwrap();
    os.flush().unwrap();
}

fn wire_fixed64(out: &mut Vec<u8>, field_number: u32, value: u64) {
    use steam_vent_proto_common::protobuf::CodedOutputStream;
    let mut os = CodedOutputStream::vec(out);
    os.write_raw_varint32((field_number << 3) | 1).unwrap();
    os.write_raw_little_endian64(value).unwrap();
    os.flush().unwrap();
}

fn wire_bytes(out: &mut Vec<u8>, field_number: u32, bytes: &[u8]) {
    use steam_vent_proto_common::protobuf::CodedOutputStream;
    let mut os = CodedOutputStream::vec(out);
    os.write_raw_varint32((field_number << 3) | 2).unwrap();
    os.write_bytes_no_tag(bytes).unwrap();
    os.flush().unwrap();
}

#[test]
fn build_query_files_proto_requests_full_description_and_normalizes_cursor() {
    use super::mapping::build_query_files_proto;

    let proto = build_query_files_proto(
        550,
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "   ",
        500,
    );
    assert_eq!(proto.appid, Some(550));
    assert_eq!(proto.cursor.as_deref(), Some("*"));
    assert_eq!(proto.numperpage, Some(100));
    assert_eq!(proto.return_short_description, Some(false));
    assert_eq!(proto.return_details, Some(true));

    let paged = build_query_files_proto(
        550,
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "  AoIIPw==  ",
        0,
    );
    assert_eq!(paged.cursor.as_deref(), Some("AoIIPw=="));
    assert_eq!(paged.numperpage, Some(1));
}

#[test]
fn decode_query_files_response_recovers_invalid_utf8_in_top_and_nested_fields() {
    use super::wire::{decode_response_lossy, QUERY_FILES_RESPONSE_SCHEMA};
    use steam_vent_proto_common::protobuf::{Message, UnknownValueRef};
    use steam_vent_proto_steam::steammessages_publishedfile_steamclient::CPublishedFile_QueryFiles_Response;

    let clean_item = PublishedFileDetails {
        result: Some(1),
        publishedfileid: Some(10001),
        title: Some("正常中文战役".into()),
        file_url: Some("https://cdn.example.test/clean.vpk".into()),
        ..Default::default()
    };

    let mut dirty_item = Vec::new();
    wire_varint(&mut dirty_item, 1, 1); // result = OK
    wire_varint(&mut dirty_item, 2, 10002); // publishedfileid
    wire_fixed64(&mut dirty_item, 3, 76561198000000001); // creator
    wire_bytes(&mut dirty_item, 7, b"legacy_\xff_path.vpk"); // filename
    wire_varint(&mut dirty_item, 8, 4096); // file_size
    wire_bytes(&mut dirty_item, 10, b"https://cdn.example.test/dirty.vpk"); // file_url
    let mut bad_title = "求生之路".as_bytes().to_vec();
    bad_title.extend_from_slice(&[0xFF, b'!']);
    wire_bytes(&mut dirty_item, 16, &bad_title); // title
                                                 // Simulate Steam's byte-boundary truncation slicing a 3-byte UTF-8 CJK character ("中" = E4 B8 AD).
    let mut truncated_short_desc = "简介前缀".as_bytes().to_vec();
    truncated_short_desc.extend_from_slice(&[0xE4, 0xB8]);
    wire_bytes(&mut dirty_item, 18, &truncated_short_desc); // short_description
    wire_bytes(&mut dirty_item, 17, b"full desc \x80 tail"); // file_description

    let mut preview = Vec::new();
    wire_varint(&mut preview, 1, 77); // previewid
    wire_bytes(&mut preview, 3, b"https://cdn.example.test/p.jpg"); // url
    wire_bytes(&mut preview, 5, b"shot_\xfe.jpg"); // filename
    wire_bytes(&mut dirty_item, 51, &preview);

    let mut tag = Vec::new();
    wire_bytes(&mut tag, 1, b"Campaigns\xff");
    wire_bytes(&mut tag, 3, b"\xe4\xb8");
    wire_bytes(&mut dirty_item, 52, &tag);

    let mut kvtag = Vec::new();
    wire_bytes(&mut kvtag, 1, b"k\x80");
    wire_bytes(&mut kvtag, 2, b"v\xff");
    wire_bytes(&mut dirty_item, 54, &kvtag);

    let mut snapshot = Vec::new();
    wire_varint(&mut snapshot, 1, 1700000000);
    wire_bytes(&mut snapshot, 2, b"branch_\xff");
    wire_bytes(&mut dirty_item, 75, &snapshot);

    // Unknown length-delimited binary field should pass through untouched.
    let unknown_raw = [0xFF, 0x00, 0xFE, 0x80];
    wire_bytes(&mut dirty_item, 99, &unknown_raw);

    let mut response_bytes = Vec::new();
    wire_varint(&mut response_bytes, 1, 2); // total
    wire_bytes(
        &mut response_bytes,
        2,
        &clean_item.write_to_bytes().unwrap(),
    );
    wire_bytes(&mut response_bytes, 2, &dirty_item);
    wire_bytes(&mut response_bytes, 3, b"AoIIPw=="); // next_cursor

    // Verify strict rust-protobuf fails with the exact production error first.
    let strict_err = CPublishedFile_QueryFiles_Response::parse_from_bytes(&response_bytes)
        .expect_err("strict protobuf parsing should fail on invalid UTF-8");
    assert!(
        strict_err.to_string().contains("Invalid UTF-8 sequence"),
        "unexpected strict error: {strict_err}"
    );

    let decoded: CPublishedFile_QueryFiles_Response = decode_response_lossy(
        "PublishedFile.QueryFiles#1",
        &response_bytes,
        QUERY_FILES_RESPONSE_SCHEMA,
    )
    .expect("lossy decoder should sanitize invalid UTF-8 and recover the whole page");

    assert_eq!(decoded.total, Some(2));
    assert_eq!(decoded.next_cursor.as_deref(), Some("AoIIPw=="));
    assert_eq!(decoded.publishedfiledetails.len(), 2);
    assert_eq!(decoded.publishedfiledetails[0].publishedfileid, Some(10001));
    assert_eq!(
        decoded.publishedfiledetails[0].title.as_deref(),
        Some("正常中文战役")
    );

    let recovered = &decoded.publishedfiledetails[1];
    assert_eq!(recovered.publishedfileid, Some(10002));
    assert_eq!(recovered.creator, Some(76561198000000001));
    assert_eq!(recovered.file_size, Some(4096));
    assert_eq!(
        recovered.file_url.as_deref(),
        Some("https://cdn.example.test/dirty.vpk")
    );
    assert_eq!(
        recovered.filename.as_deref(),
        Some("legacy_\u{FFFD}_path.vpk")
    );
    assert_eq!(recovered.title.as_deref(), Some("求生之路\u{FFFD}!"));
    assert_eq!(
        recovered.short_description.as_deref(),
        Some("简介前缀\u{FFFD}")
    );
    assert_eq!(
        recovered.file_description.as_deref(),
        Some("full desc \u{FFFD} tail")
    );
    assert_eq!(
        recovered.previews[0].filename.as_deref(),
        Some("shot_\u{FFFD}.jpg")
    );
    assert_eq!(recovered.tags[0].tag.as_deref(), Some("Campaigns\u{FFFD}"));
    assert_eq!(recovered.tags[0].display_name.as_deref(), Some("\u{FFFD}"));
    assert_eq!(recovered.kvtags[0].key.as_deref(), Some("k\u{FFFD}"));
    assert_eq!(recovered.kvtags[0].value.as_deref(), Some("v\u{FFFD}"));
    assert_eq!(
        recovered.author_snapshots[0].game_branch_min.as_deref(),
        Some("branch_\u{FFFD}")
    );
    assert_eq!(
        recovered.special_fields.unknown_fields().get(99),
        Some(UnknownValueRef::LengthDelimited(&unknown_raw))
    );
}

#[test]
fn decode_get_details_response_recovers_invalid_utf8_without_losing_batch() {
    use super::wire::{decode_response_lossy, GET_DETAILS_RESPONSE_SCHEMA};
    use steam_vent_proto_common::protobuf::Message;
    use steam_vent_proto_steam::steammessages_publishedfile_steamclient::CPublishedFile_GetDetails_Response;

    let mut dirty = Vec::new();
    wire_varint(&mut dirty, 1, 1);
    wire_varint(&mut dirty, 2, 20001);
    wire_varint(&mut dirty, 28, 1); // banned = true
    wire_bytes(&mut dirty, 29, b"DMCA \xff notice"); // ban_reason
    wire_bytes(&mut dirty, 58, b"meta\xfe"); // metadata

    let clean = PublishedFileDetails {
        result: Some(1),
        publishedfileid: Some(20002),
        title: Some("Clean Mod".into()),
        ..Default::default()
    };

    let mut response_bytes = Vec::new();
    wire_bytes(&mut response_bytes, 1, &dirty);
    wire_bytes(&mut response_bytes, 1, &clean.write_to_bytes().unwrap());

    assert!(CPublishedFile_GetDetails_Response::parse_from_bytes(&response_bytes).is_err());

    let decoded: CPublishedFile_GetDetails_Response = decode_response_lossy(
        "PublishedFile.GetDetails#1",
        &response_bytes,
        GET_DETAILS_RESPONSE_SCHEMA,
    )
    .expect("GetDetails batch should recover");

    assert_eq!(decoded.publishedfiledetails.len(), 2);
    assert_eq!(
        decoded.publishedfiledetails[0].ban_reason.as_deref(),
        Some("DMCA \u{FFFD} notice")
    );
    assert_eq!(
        decoded.publishedfiledetails[0].metadata.as_deref(),
        Some("meta\u{FFFD}")
    );
    assert_eq!(decoded.publishedfiledetails[1].publishedfileid, Some(20002));
}

#[test]
fn decode_player_link_details_preserves_binary_avatar_digest_while_sanitizing_strings() {
    use super::wire::{decode_response_lossy, GET_PLAYER_LINK_DETAILS_RESPONSE_SCHEMA};
    use steam_vent_proto_common::protobuf::Message;
    use steam_vent_proto_steam::steammessages_player_steamclient::CPlayer_GetPlayerLinkDetails_Response;

    let avatar_digest = vec![0xFF, 0xFE, 0x80, 0x01, 0x02, 0xC3, 0x28];
    let mut public_data = Vec::new();
    wire_fixed64(&mut public_data, 1, 76561198000000042); // required steamid
    wire_bytes(&mut public_data, 9, &avatar_digest); // bytes sha_digest_avatar
    wire_bytes(&mut public_data, 10, b"Player\xffName"); // string persona_name

    let mut private_data = Vec::new();
    wire_bytes(&mut private_data, 8, b"L4D2 \xfe Lobby"); // string game_extra_info

    let mut account = Vec::new();
    wire_bytes(&mut account, 1, &public_data);
    wire_bytes(&mut account, 2, &private_data);

    let mut response_bytes = Vec::new();
    wire_bytes(&mut response_bytes, 1, &account);

    assert!(CPlayer_GetPlayerLinkDetails_Response::parse_from_bytes(&response_bytes).is_err());

    let decoded: CPlayer_GetPlayerLinkDetails_Response = decode_response_lossy(
        "Player.GetPlayerLinkDetails#1",
        &response_bytes,
        GET_PLAYER_LINK_DETAILS_RESPONSE_SCHEMA,
    )
    .expect("PlayerLinkDetails should recover");

    assert_eq!(decoded.accounts.len(), 1);
    let acc = &decoded.accounts[0];
    assert_eq!(acc.public_data.steamid(), 76561198000000042);
    assert_eq!(
        acc.public_data.sha_digest_avatar.as_deref(),
        Some(avatar_digest.as_slice())
    );
    assert_eq!(
        acc.public_data.persona_name.as_deref(),
        Some("Player\u{FFFD}Name")
    );
    assert_eq!(
        acc.private_data.game_extra_info.as_deref(),
        Some("L4D2 \u{FFFD} Lobby")
    );
}

#[test]
fn decode_response_lossy_still_rejects_structurally_corrupt_payload() {
    use super::wire::{decode_response_lossy, QUERY_FILES_RESPONSE_SCHEMA};
    use steam_vent::NetworkError;
    use steam_vent_proto_steam::steammessages_publishedfile_steamclient::CPublishedFile_QueryFiles_Response;

    // Truncated string field (field 3 `next_cursor` claiming 50 bytes with only 2 present).
    let truncated_string = vec![0x1A, 50, b'a', b'b'];
    let err = decode_response_lossy::<CPublishedFile_QueryFiles_Response>(
        "PublishedFile.QueryFiles#1",
        &truncated_string,
        QUERY_FILES_RESPONSE_SCHEMA,
    )
    .unwrap_err();
    assert!(matches!(err, NetworkError::MalformedBody(_)));

    // Truncated varint (continuation bit set at EOF).
    let truncated_varint = vec![0x08, 0x80];
    let err = decode_response_lossy::<CPublishedFile_QueryFiles_Response>(
        "PublishedFile.QueryFiles#1",
        &truncated_varint,
        QUERY_FILES_RESPONSE_SCHEMA,
    )
    .unwrap_err();
    assert!(matches!(err, NetworkError::MalformedBody(_)));

    // Invalid UTF-8 in field 3 triggers the sanitizer, followed by a truncated field 3 that the
    // sanitizer itself must reject instead of silently dropping.
    let utf8_then_truncated = vec![0x1A, 3, b'a', 0xFF, b'b', 0x1A, 50, b'x', b'y'];
    let err = decode_response_lossy::<CPublishedFile_QueryFiles_Response>(
        "PublishedFile.QueryFiles#1",
        &utf8_then_truncated,
        QUERY_FILES_RESPONSE_SCHEMA,
    )
    .unwrap_err();
    assert!(matches!(err, NetworkError::MalformedBody(_)));
}

#[test]
fn raw_service_method_roundtrips_through_steam_vent_raw_net_message() {
    use super::mapping::build_query_files_proto;
    use super::wire::{
        RawServiceMethodRequest, RawServiceMethodResponse, QUERY_FILES_RESPONSE_SCHEMA,
    };
    use bytes::BytesMut;
    use steam_vent::{NetMessageHeader, NetworkError, RawNetMessage};
    use steam_vent_proto_common::MsgKindEnum;
    use steam_vent_proto_steam::enums_clientserver::EMsg;

    let req = build_query_files_proto(550, QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE, 0, "*", 50);
    let raw_req =
        RawNetMessage::from_message(NetMessageHeader::default(), RawServiceMethodRequest(req))
            .expect("RawServiceMethodRequest should serialize");
    assert_eq!(raw_req.kind, EMsg::k_EMsgServiceMethodCallFromClient);
    assert!(raw_req.is_protobuf);
    assert_eq!(
        raw_req.header.target_job_name.as_deref(),
        Some("PublishedFile.QueryFiles#1")
    );

    let mut dirty_item = Vec::new();
    wire_varint(&mut dirty_item, 1, 1);
    wire_varint(&mut dirty_item, 2, 30001);
    wire_bytes(&mut dirty_item, 16, b"Broken\xffTitle");
    let mut body = Vec::new();
    wire_varint(&mut body, 1, 1);
    wire_bytes(&mut body, 2, &dirty_item);

    // Encode a genuine EMsg::k_EMsgServiceMethodResponse (147) frame as Steam CM sends it.
    let build_frame = |eresult: u64| {
        let mut frame = Vec::new();
        let kind_wire = EMsg::k_EMsgServiceMethodResponse.encode_kind(true);
        frame.extend_from_slice(&kind_wire.to_le_bytes());
        let mut proto_hdr = Vec::new();
        wire_bytes(&mut proto_hdr, 12, b"PublishedFile.QueryFiles#1"); // target_job_name
        wire_varint(&mut proto_hdr, 13, eresult); // eresult
        frame.extend_from_slice(&(proto_hdr.len() as u32).to_le_bytes());
        frame.extend_from_slice(&proto_hdr);
        frame.extend_from_slice(&body);
        frame
    };

    let parsed_raw = RawNetMessage::read(build_frame(EResult::OK as u64)).unwrap();
    assert_eq!(parsed_raw.kind, EMsg::k_EMsgServiceMethodResponse);
    assert_eq!(parsed_raw.kind.value(), 147);
    let resp_msg = parsed_raw
        .into_message::<RawServiceMethodResponse>()
        .unwrap();
    let decoded = resp_msg
        .into_response::<CPublishedFile_QueryFiles_Request>(QUERY_FILES_RESPONSE_SCHEMA)
        .unwrap();
    assert_eq!(decoded.publishedfiledetails.len(), 1);
    assert_eq!(
        decoded.publishedfiledetails[0].title.as_deref(),
        Some("Broken\u{FFFD}Title")
    );

    // Non-OK EResult in CMsgProtoBufHeader still surfaces as NetworkError::ApiError.
    let api_err = RawNetMessage::read(build_frame(EResult::Fail as u64))
        .unwrap()
        .into_message::<RawServiceMethodResponse>()
        .unwrap_err();
    assert!(matches!(api_err, NetworkError::ApiError(EResult::Fail)));

    // Method mismatch still produces NetworkError::DifferentServiceMethod.
    let mismatch = RawServiceMethodResponse {
        job_name: "PublishedFile.GetDetails#1".into(),
        body: BytesMut::from(body.as_slice()),
    }
    .into_response::<CPublishedFile_QueryFiles_Request>(QUERY_FILES_RESPONSE_SCHEMA)
    .unwrap_err();
    assert!(matches!(
        mismatch,
        NetworkError::DifferentServiceMethod("PublishedFile.QueryFiles#1", _)
    ));
}
