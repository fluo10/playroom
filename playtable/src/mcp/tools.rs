// MCP ツール定義（rmcp の #[tool] マクロを使用）
// 実装は bridge.rs の PlaytableBridge を通じてゲームサーバーと通信する

// TODO: rmcp の API に合わせて実装
// 例:
// #[tool(description = "Flip a coin")]
// async fn flip_coin(&self) -> CallToolResult { ... }
//
// #[tool(description = "Roll a dice")]
// async fn roll_dice(&self, sides: u8) -> CallToolResult { ... }
//
// #[tool(description = "Get current game state")]
// async fn get_state(&self) -> CallToolResult { ... }
