#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use midnight_run_tools::tools::{Tool, ToolResult, automation_attack::AutomationAttack};

fn main() -> ToolResult {
    let mut tool = AutomationAttack;
    tool.run()
}
