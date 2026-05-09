#[derive(Debug, Clone)]
pub struct LspServerDef {
    pub language: &'static str,
    pub display_name: &'static str,
    pub extensions: &'static [&'static str],
    pub binary: &'static str,
    pub start_args: &'static [&'static str],
    pub install_cmd: &'static str,
    pub install_note: &'static str,
}

pub const SERVERS: &[LspServerDef] = &[
    LspServerDef {
        language: "rust",
        display_name: "Rust",
        extensions: &["rs"],
        binary: "rust-analyzer",
        start_args: &[],
        install_cmd: "rustup component add rust-analyzer",
        install_note: "Requires Rust / rustup",
    },
    LspServerDef {
        language: "typescript",
        display_name: "TypeScript / JavaScript",
        extensions: &["ts", "tsx", "js", "jsx", "mjs"],
        binary: "typescript-language-server",
        start_args: &["--stdio"],
        install_cmd: "npm install -g typescript-language-server typescript",
        install_note: "Requires Node.js / npm",
    },
    LspServerDef {
        language: "python",
        display_name: "Python",
        extensions: &["py", "pyi"],
        binary: "pyright-langserver",
        start_args: &["--stdio"],
        install_cmd: "pip install pyright",
        install_note: "Requires Python / pip",
    },
    LspServerDef {
        language: "go",
        display_name: "Go",
        extensions: &["go"],
        binary: "gopls",
        start_args: &[],
        install_cmd: "go install golang.org/x/tools/gopls@latest",
        install_note: "Requires Go",
    },
    LspServerDef {
        language: "cpp",
        display_name: "C / C++",
        extensions: &["c", "h", "cpp", "cc", "cxx", "hpp"],
        binary: "clangd",
        start_args: &[],
        install_cmd: "# Linux: sudo apt install clangd  |  macOS: brew install llvm",
        install_note: "Requires clang toolchain",
    },
    LspServerDef {
        language: "csharp",
        display_name: "C#",
        extensions: &["cs"],
        binary: "csharp-ls",
        start_args: &[],
        install_cmd: "dotnet tool install -g csharp-ls",
        install_note: "Requires .NET SDK",
    },
    LspServerDef {
        language: "lua",
        display_name: "Lua",
        extensions: &["lua"],
        binary: "lua-language-server",
        start_args: &[],
        install_cmd: "# macOS: brew install lua-language-server  |  see luals.github.io",
        install_note: "See luals.github.io",
    },
];

pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    for server in SERVERS {
        if server.extensions.contains(&ext) {
            return Some(server.language);
        }
    }
    None
}

pub fn server_for_language(language: &str) -> Option<&'static LspServerDef> {
    SERVERS.iter().find(|s| s.language == language)
}

pub fn check_installed(def: &LspServerDef) -> bool {
    std::process::Command::new("which")
        .arg(def.binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
