pub mod automation_attack;
pub mod boot_breaker;
pub mod launcher;

pub type ToolError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type ToolResult<T = ()> = Result<T, ToolError>;

pub trait Tool {
    fn name(&self) -> &'static str;

    fn run(&mut self) -> ToolResult;
}
