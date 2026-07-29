#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use midnight_run_tools::tools::{Tool, ToolResult, boot_breaker::BootBreaker};

fn main() -> ToolResult {
    let mut tool = BootBreaker;
    tool.run()
}
