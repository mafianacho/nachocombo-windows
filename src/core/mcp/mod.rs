//! NachoCombo MCP for Windows (port of the macOS NachoCombo MCP server, `docs/NACHOCOMBO-MCP.md`): the gadget tools of
//! the connector registry offered to MCP clients on this PC (`tools/list`, `tools/call`), behind an API key, on
//! `http://127.0.0.1:47653/mcp` only. Everything here is platform-free and tested on Linux; the Windows shell
//! (`src/mcp.rs`) wires the executor, the pet's approval card and Settings ▸ MCP.
pub mod http;
pub mod jsonrpc;
pub mod keys;
pub mod limits;
pub mod policy;
pub mod server;
pub mod service;
pub mod settings;
pub mod tools;
