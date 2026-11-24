use super::auth::{ActiveFlowHandle, SteamLoginConfirmation};
use super::mapping::map_detail;
use super::*;
use crate::config::CatalogConfigStore;
use l4a_protocol::{SteamAuthFlowKind, SteamAuthFlowState};
use std::sync::Arc;
use steam_vent::auth::{
    AuthConfirmationHandler, ConfirmationAction, UserProvidedAuthConfirmationHandler,
};
use steam_vent::EResult;
use steam_vent_proto_common::protobuf::EnumOrUnknown;
use steam_vent_proto_steam::steammessages_publishedfile_steamclient::PublishedFileDetails;

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
