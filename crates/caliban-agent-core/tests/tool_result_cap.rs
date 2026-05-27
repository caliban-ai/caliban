//! `ToolResultCap` overflow + idempotency tests.

#![allow(missing_docs)]

use caliban_agent_core::post_process::ToolResultCap;
use caliban_provider::{ContentBlock, TextBlock, ToolResultBlock};
use tempfile::tempdir;

#[tokio::test]
async fn caps_oversized_block_and_writes_overflow() {
    let dir = tempdir().unwrap();
    let cap = ToolResultCap {
        max_chars: 50,
        overflow_dir: dir.path().into(),
        session_id: "sess".into(),
    };
    let huge: String = "x".repeat(500);
    let mut blocks = vec![ContentBlock::ToolResult(ToolResultBlock {
        tool_use_id: "tu_1".into(),
        content: vec![ContentBlock::Text(TextBlock {
            text: huge.clone(),
            cache_control: None,
        })],
        is_error: false,
    })];
    let n = cap.cap(&mut blocks).await.unwrap();
    assert_eq!(n, 1);
    // Placeholder content
    let body = match &blocks[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert!(body.starts_with("[truncated: 500 chars"));
    assert!(body.contains(dir.path().to_string_lossy().as_ref()));
    // Overflow file exists with the original
    let overflow_path = dir.path().join("sess").join("tu_1.txt");
    let overflow = std::fs::read_to_string(&overflow_path).unwrap();
    assert!(overflow.contains(&huge));
}

#[tokio::test]
async fn small_blocks_pass_through_untouched() {
    let dir = tempdir().unwrap();
    let cap = ToolResultCap {
        max_chars: 1000,
        overflow_dir: dir.path().into(),
        session_id: "sess".into(),
    };
    let mut blocks = vec![ContentBlock::ToolResult(ToolResultBlock {
        tool_use_id: "tu_1".into(),
        content: vec![ContentBlock::Text(TextBlock {
            text: "small".into(),
            cache_control: None,
        })],
        is_error: false,
    })];
    let n = cap.cap(&mut blocks).await.unwrap();
    assert_eq!(n, 0);
    let body = match &blocks[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert_eq!(body, "small");
}
