//! Playroom 共通のロビー/メインメニュー GUI（Bevy ベース）。
//!
//! このモジュールは `gui` feature 下でのみ有効。アプリ側は
//! `PlayroomLobbyPlugin` を Bevy App に追加することでロビー機能が使える。

#![cfg(feature = "gui")]

use std::sync::Arc;

use bevy::prelude::*;
use bevy_simple_text_input::TextInputPlugin;
use tokio::sync::Mutex;

use crate::service::AppService;
use crate::AppConfig;

mod bridge;
mod screens;
mod state;

pub use state::AppScreen;

/// Playroom 共通のロビー GUI プラグイン。
///
/// アプリ側では：
/// ```ignore
/// App::new()
///     .add_plugins(DefaultPlugins)
///     .add_plugins(playroom::gui::PlayroomLobbyPlugin::default())
///     .run();
/// ```
///
/// このプラグインが追加/管理するリソース：
/// - [`ServiceHandle`] — AppService + Tokio runtime
/// - [`AppScreen`] State — Bootstrap / MainMenu / InRoom
/// - [`ServiceStatus`] — 直近の操作結果テキスト（画面下端等に表示）
#[derive(Default)]
pub struct PlayroomLobbyPlugin;

impl Plugin for PlayroomLobbyPlugin {
    fn build(&self, app: &mut App) {
        // Tokio ランタイムと AppService を起動
        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let config = Arc::new(Mutex::new(AppConfig::load_or_default()));
        let (service, event_rx) = runtime
            .block_on(AppService::new(config))
            .expect("failed to start AppService");

        let handle = bridge::ServiceHandle::new(Arc::new(runtime), service);
        // Tokio 側 AppEvent → std::sync::mpsc に中継（Bevy 側が polling で消費）
        let incoming = bridge::spawn_event_forwarder(&handle, event_rx);

        app.insert_resource(handle)
            .insert_resource(incoming)
            .insert_resource(state::ServiceStatus::default())
            .init_state::<AppScreen>()
            .add_plugins(TextInputPlugin)
            .add_plugins(screens::ScreensPlugin)
            .add_systems(Update, bridge::poll_app_events)
            .add_systems(Update, bridge::poll_service_results);
    }
}
