//! 画面間で共用するユーティリティ：ボタン、ラベル、ステータスバー。

use bevy::prelude::*;
use bevy_simple_text_input::{TextInput, TextInputTextColor, TextInputTextFont, TextInputValue};

use super::super::state::ServiceStatus;

pub const COLOR_BG: Color = Color::srgb(0.10, 0.10, 0.12);
pub const COLOR_PANEL: Color = Color::srgb(0.18, 0.18, 0.22);
pub const COLOR_TEXT: Color = Color::srgb(0.92, 0.92, 0.92);
pub const COLOR_DIM: Color = Color::srgb(0.70, 0.70, 0.74);
pub const COLOR_BORDER: Color = Color::srgb(0.40, 0.45, 0.60);
pub const COLOR_BUTTON_NORMAL: Color = Color::srgb(0.20, 0.24, 0.32);
pub const COLOR_BUTTON_HOVER: Color = Color::srgb(0.26, 0.32, 0.42);
pub const COLOR_BUTTON_PRESS: Color = Color::srgb(0.14, 0.18, 0.24);

/// フルスクリーンのルートノード（縦並び）。
pub fn root_node() -> Node {
    Node {
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Stretch,
        padding: UiRect::all(Val::Px(16.0)),
        row_gap: Val::Px(12.0),
        ..default()
    }
}

/// ボタン（テキスト付き）を spawn するヘルパー。返り値はボタンの Entity。
pub fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    extra: impl Bundle,
) -> Entity {
    parent
        .spawn((
            Button,
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                min_width: Val::Px(160.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(COLOR_BUTTON_NORMAL),
            BorderColor::all(COLOR_BORDER),
            extra,
        ))
        .with_children(|b| {
            b.spawn((Text::new(label), TextColor(COLOR_TEXT)));
        })
        .id()
}

/// ボタンの背景色をインタラクションで切り替える共通 system。
pub fn update_button_colors(
    mut q: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<Button>)>,
) {
    for (interaction, mut bg) in &mut q {
        *bg = match *interaction {
            Interaction::Pressed => BackgroundColor(COLOR_BUTTON_PRESS),
            Interaction::Hovered => BackgroundColor(COLOR_BUTTON_HOVER),
            Interaction::None => BackgroundColor(COLOR_BUTTON_NORMAL),
        };
    }
}

/// テキスト入力ウィジェットを spawn する。初期値は空。
pub fn spawn_text_input(
    parent: &mut ChildSpawnerCommands,
    placeholder_hint: &str,
    extra: impl Bundle,
) -> Entity {
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                border: UiRect::all(Val::Px(1.0)),
                padding: UiRect::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(COLOR_PANEL),
            BorderColor::all(COLOR_BORDER),
            TextInput,
            TextInputValue(String::new()),
            TextInputTextFont(TextFont {
                font_size: 18.0,
                ..default()
            }),
            TextInputTextColor(TextColor(COLOR_TEXT)),
            extra,
        ))
        .with_children(|b| {
            // プレースホルダは別途テキストで説明として表示
            b.spawn((
                Text::new(format!("({placeholder_hint})")),
                TextColor(COLOR_DIM),
                Node {
                    display: Display::None, // 現状は非表示（マウスで focus される前提）
                    ..default()
                },
            ));
        })
        .id()
}

/// 小さなラベル。
pub fn label(parent: &mut ChildSpawnerCommands, text: &str) {
    parent.spawn((Text::new(text), TextColor(COLOR_TEXT)));
}

pub fn dim_label(parent: &mut ChildSpawnerCommands, text: &str) {
    parent.spawn((Text::new(text), TextColor(COLOR_DIM)));
}

/// ServiceStatus を画面下端の小さなラベルに反映する system。
/// ステータスバーの Entity は各画面の setup で spawn する。
#[derive(Component)]
pub struct StatusBarText;

pub fn update_status_bar(
    status: Res<ServiceStatus>,
    mut q: Query<&mut Text, With<StatusBarText>>,
) {
    if !status.is_changed() {
        return;
    }
    for mut text in &mut q {
        **text = status.text.clone();
    }
}

