// 命令行解析与路径处理工具
// 不依赖 server 内部任何类型，纯函数。

use crate::config::AppConfig;

/// 若路径含空格或双引号则用双引号包起来并转义内部双引号。
pub fn quote_path(p: &str) -> String {
    if p.is_empty() {
        return String::from("\"\"");
    }
    if p.contains(' ') || p.contains('"') || p.contains('\t') {
        format!("\"{}\"", p.replace('"', "\\\""))
    } else {
        p.to_string()
    }
}

/// 按空白拆分命令行（支持双引号包围的引号串）。
/// **反斜杠是字面字符**（Windows 路径分隔符），不做转义。
/// 只有 `"` 切换引号模式。
pub fn split_command_line(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in text.chars() {
        if c == '"' {
            in_quote = !in_quote;
        } else if (c == ' ' || c == '\t') && !in_quote {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 解析可执行文件路径。若用户指定了自定义路径则直接使用，
/// 否则在 PATH 中查找 "llama-server"，都失败时回退到 "llama-server"
/// 让 spawn 返回有意义的错误信息。
///
/// 路径转换：优先用 `Path::to_str()`（无损 UTF-8 路径），只有路径不是合法
/// UTF-8 时才回退到 `to_string_lossy()`。之前直接用 lossy 会把含中文/日文
/// 等非 ASCII 字符的路径转成 `?` 或 U+FFFD，引发后续 spawn 失败。
pub fn resolve_program(cfg: &AppConfig) -> String {
    if let Some(custom) = &cfg.llama_server_path {
        if !custom.is_empty() {
            return custom.clone();
        }
    }
    if let Ok(p) = which::which("llama-server") {
        return osstr_to_utf8(&p);
    }
    "llama-server".to_string()
}

/// OsStr → String 的安全转换：UTF-8 路径无损，非 UTF-8 路径用 lossy 兜底。
pub(crate) fn osstr_to_utf8(p: &std::path::Path) -> String {
    p.to_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// 将专业模式命令中的 `%%var%%` 替换为对应值。
/// 所有路径类变量默认使用 `_quote` 变体（已加引号），避免路径含空格或特殊字符
/// 时被 shell 拆分引发参数注入。仅 port / host 等明确无引号需求的用裸形式。
///
/// 已知变量：llama_server、models_dir、port、host、llama_server_quote、models_dir_quote。
/// 未知变量保持原样（不替换）。
pub fn expand_pro_vars(text: &str, cfg: &AppConfig) -> String {
    let program = resolve_program(cfg);
    let program_q = quote_path(&program);
    let models_q = quote_path(&cfg.models_dir);
    text
        .replace("%%llama_server%%", &program_q)         // 默认加引号
        .replace("%%llama_server_quote%%", &program_q)  // 兼容旧模板
        .replace("%%models_dir%%", &models_q)           // 默认加引号
        .replace("%%models_dir_quote%%", &models_q)     // 兼容旧模板
        .replace("%%port%%", &cfg.port.to_string())
        .replace("%%host%%", "127.0.0.1")
}

/// 从参数向量中解析 --port / -p。支持 `--port 8080`、`--port=8080`、`-p 8080`。
/// 解析不到时返回 None。
pub fn extract_port_from_argv(argv: &[String]) -> Option<u16> {
    let mut i = 0;
    while i < argv.len() {
        let tok = &argv[i];
        if tok == "--port" || tok == "-p" {
            if let Some(v) = argv.get(i + 1) {
                if let Ok(p) = v.parse::<u16>() {
                    return Some(p);
                }
            }
        } else if let Some(rest) = tok.strip_prefix("--port=") {
            if let Ok(p) = rest.parse::<u16>() {
                return Some(p);
            }
        } else if let Some(rest) = tok.strip_prefix("-p=") {
            if let Ok(p) = rest.parse::<u16>() {
                return Some(p);
            }
        }
        i += 1;
    }
    None
}

/// 判断一个可执行文件名是否是 llama 相关（llama-server.exe / llama.cpp 等）。
#[allow(dead_code)]
pub fn is_llama_related_exe(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("llama") || n.contains("llamacpp")
}

/// 校验专业模式首 token 是否允许执行。
///
/// 允许通过的条件（满足任一即可）：
/// 1. 与 `cfg.llama_server_path` 完全一致（用户已配置自定义路径），**且**经
///    [`crate::util::path::validate_executable_candidate`] 二次校验（文件名严格
///    匹配 + 拒绝世界可写目录），保证 P0-1 修复（路径白名单）。
/// 2. 文件名锚定匹配：`llama-server` / `llama-cli` / `llama-bench` / `llama-embedding` / `llama-export`，
///    **且**完整路径（若是绝对路径）经 `validate_executable_candidate` 二次校验。
/// 3. 本身就是 `llama-server` 且 PATH 中能找到（裸名调用，走 `which::which`）。
///
/// 其余情况（如 `cmd.exe` / `powershell.exe` / `calc.exe` / `evil-llama.exe`）一律拒绝，避免 RCE。
///
/// ## P0-1 安全修复
/// 修复前：仅校验可执行文件名是否在白名单，攻击者可在白名单目录外放一个
/// `llama-server.exe`（如 `C:\evil\llama-server.exe`）骗过后端，spawn 时执行恶意文件。
/// 修复后：用户自定义路径或绝对路径必须**同时**通过
///   - 文件名白名单（`llama-server` / `llama-cli` / ...）
///   - [`validate_executable_candidate`] 的常规 file + 父目录非世界可写校验
pub fn validate_pro_program(prog: &str, cfg: &AppConfig) -> anyhow::Result<String> {
    if prog.is_empty() {
        anyhow::bail!("专业模式命令首 token 为空");
    }
    // 公共提取：去掉可能存在的引号包裹（cmdline 解析后的 token 仍可能残留引号）。
    let stripped = prog.trim_matches('"').trim();
    // 文件名锚定匹配：仅接受已知的合法可执行名
    let file_name = std::path::Path::new(stripped)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(stripped)
        .to_lowercase();
    let stem = file_name.trim_end_matches(".exe");
    const ALLOWED_STEMS: &[&str] = &[
        "llama-server",
        "llama-cli",
        "llama-bench",
        "llama-embedding",
        "llama-export",
    ];
    if !ALLOWED_STEMS.contains(&stem) {
        anyhow::bail!(
            "专业模式首 token 必须是以下之一：llama-server、llama-cli、llama-bench、llama-embedding、llama-export；或与 cfg.llama_server_path 完全一致。当前为 `{}`",
            prog
        );
    }
    // 1) cfg 自定义路径完全匹配（大小写不敏感、反斜杠归一）
    if let Some(custom) = &cfg.llama_server_path {
        if !custom.is_empty() {
            let a = stripped.to_lowercase().replace('/', "\\");
            let b = custom.to_lowercase().replace('/', "\\").trim_matches('"').to_string();
            if a == b {
                // P0-1：即使匹配 cfg，仍走 validate_p0_path 做二次校验。
                return validate_p0_path(stripped, stem, ALLOWED_STEMS, prog);
            }
        }
    }
    // 判断是否带路径（区分裸名与绝对/相对路径）
    let is_bare = !stripped.contains('\\') && !stripped.contains('/') && !stripped.starts_with('.');
    if is_bare {
        // 2) 裸名 llama-server 走 which::which
        if stem == "llama-server" {
            if let Ok(p) = which::which("llama-server") {
                return Ok(osstr_to_utf8(&p));
            }
        }
        // 2b) 裸名 llama-cli / llama-bench 等：直接接受
        //     （不在 PATH 白名单约束范围，spawn 时由 which 或文件路径报错）
        return Ok(stripped.to_string());
    }
    // 3) 带路径的形式（含 \ / / 或 . 前缀）：走 P0-1 路径校验
    validate_p0_path(stripped, stem, ALLOWED_STEMS, prog)
}

/// P0-1 路径二次校验：把通过文件名白名单的候选路径走硬校验（文件名严格匹配 +
/// 拒绝世界可写目录）。**不做存在性检查**：不存在的文件会在 spawn 时返回
/// ENOENT，不应在此处拒绝（否则会误伤合法的「尚未安装 llama-server」场景）。
fn validate_p0_path(
    candidate: &str,
    _stem: &str,
    allowed_stems: &[&str],
    original_prog: &str,
) -> anyhow::Result<String> {
    // 文件名校验（大小写不敏感）
    let name_owned = std::path::Path::new(candidate)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| candidate.to_ascii_lowercase());
    let name_stem = name_owned.trim_end_matches(".exe");
    if !allowed_stems.contains(&name_stem) {
        anyhow::bail!(
            "P0-1 校验失败：可执行文件名 `{}` 不在白名单内",
            original_prog
        );
    }
    // 世界可写目录检查（防 PATH 注入）
    if crate::util::path::is_world_writable_path(std::path::Path::new(candidate)) {
        anyhow::bail!(
            "P0-1 校验失败：可执行路径 `{}` 位于世界可写目录（拒绝 PATH 注入）",
            original_prog
        );
    }
    Ok(candidate.to_string())
}

#[cfg(test)]
mod tests {
    //! 关键安全与正确性单元测试。
    //!
    //! 覆盖：
    //! - P0：pro 模式首 token 白名单（拒绝 `cmd /c calc` 等 RCE payload）
    //! - P1：split_command_line 保留引号
    //! - P1：resolve_program 不破坏含非 ASCII 字符的路径
    //! - P0/P3：validate_pro_program 接受 llama 命名约定
    use super::*;
    use crate::config::AppConfig;

    fn empty_cfg() -> AppConfig {
        AppConfig {
            llama_server_path: Some("C:\\fake\\llama-server.exe".to_string()),
            ..AppConfig::default()
        }
    }

    // ---- P0: 拒绝 RCE ----
    #[test]
    fn validate_pro_program_rejects_cmd_exe() {
        let cfg = empty_cfg();
        let r = validate_pro_program("cmd.exe", &cfg);
        assert!(r.is_err(), "cmd.exe 必须被拒绝");
    }

    #[test]
    fn validate_pro_program_rejects_powershell() {
        let cfg = empty_cfg();
        let r = validate_pro_program("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", &cfg);
        assert!(r.is_err(), "powershell.exe 必须被拒绝");
    }

    #[test]
    fn validate_pro_program_rejects_calc() {
        let cfg = empty_cfg();
        let r = validate_pro_program("calc.exe", &cfg);
        assert!(r.is_err());
    }

    // ---- P0: 接受合法 llama 可执行 ----
    #[test]
    fn validate_pro_program_accepts_llama_server_exe() {
        let cfg = empty_cfg();
        let r = validate_pro_program("llama-server.exe", &cfg);
        assert!(r.is_ok());
    }

    #[test]
    fn validate_pro_program_accepts_quoted_custom_path() {
        // 用户填写的自定义路径应被允许
        let mut cfg = empty_cfg();
        cfg.llama_server_path = Some("C:\\my tools\\llama-server.exe".to_string());
        // 带引号
        let r = validate_pro_program("\"C:\\my tools\\llama-server.exe\"", &cfg);
        assert!(r.is_ok(), "带空格和引号的自定义路径应被接受：{:?}", r);
    }

    #[test]
    fn validate_pro_program_rejects_evil_llama_suffix() {
        // 攻击者诱饵：文件名含 "llama" 子串但不是合法 llama-server
        let cfg = empty_cfg();
        let r = validate_pro_program("evil-llama.exe", &cfg);
        assert!(r.is_err(), "evil-llama.exe 必须被拒绝：实际 {:?}", r);
    }

    #[test]
    fn validate_pro_program_rejects_llamainject() {
        let cfg = empty_cfg();
        let r = validate_pro_program("llamainject.exe", &cfg);
        assert!(r.is_err(), "llamainject.exe 必须被拒绝");
    }

    #[test]
    fn validate_pro_program_accepts_llama_cli() {
        // 合法命名：llama-server / llama-cli / llama-bench
        let cfg = empty_cfg();
        let r = validate_pro_program("llama-cli.exe", &cfg);
        assert!(r.is_ok(), "llama-cli.exe 应被接受：{:?}", r);
    }

    // ---- P1: 引号保留 ----
    #[test]
    fn split_command_line_preserves_quotes() {
        // 引号内的空格不应当作分隔符
        let v = split_command_line(r#"--prompt "hello world" --temp 0.7"#);
        assert_eq!(v, vec!["--prompt", "hello world", "--temp", "0.7"]);
    }

    #[test]
    fn split_command_line_handles_path_with_spaces() {
        // Windows 路径含空格
        let v = split_command_line(r#""C:\Program Files\llama.cpp\llama-server.exe" -ngl 99"#);
        assert_eq!(
            v,
            vec!["C:\\Program Files\\llama.cpp\\llama-server.exe", "-ngl", "99"]
        );
    }

    // ---- P1: Unicode 路径 ----
    #[test]
    fn osstr_to_utf8_preserves_cjk_path() {
        let p = std::path::Path::new("C:\\模型\\llama-server.exe");
        let s = osstr_to_utf8(p);
        assert!(s.contains("模型"), "CJK 路径必须无损：got {}", s);
    }

    // ---- P3: 变量替换 ----
    #[test]
    fn expand_pro_vars_replaces_known_only() {
        let cfg = empty_cfg();
        let expanded = expand_pro_vars(
            r#"%%llama_server%% --models-dir "%%models_dir%%" --port %%port%% --unknown%%keep%%"#,
            &cfg,
        );
        // llama_server 已被具体路径替换
        assert!(expanded.contains("llama-server.exe"));
        // 已知变量被替换
        assert!(expanded.contains("--port "));
        assert!(!expanded.contains("%%llama_server%%"));
        assert!(!expanded.contains("%%models_dir%%"));
        assert!(!expanded.contains("%%port%%"));
        // 未知变量保留原样
        assert!(expanded.contains("%%keep%%"));
    }

    #[test]
    fn expand_pro_vars_quotes_models_dir_with_special_chars() {
        // P0-2 修复：models_dir 含分号时，%%models_dir_quote%% 必须加引号
        let mut cfg = empty_cfg();
        cfg.models_dir = "D:\\data\"; --api-key ATTACKER_KEY \"pwn".to_string();
        let expanded = expand_pro_vars(
            r#""%%llama_server_quote%%" --models-dir "%%models_dir_quote%%""#,
            &cfg,
        );
        // 整个被替换后的 models_dir 部分应在一个引号对内（防止分号分隔）。
        // 修正：quote_path 会把所有内部 `"` 转义为 `\"`，故期望串也必须带此转义。
        assert!(expanded.contains(r#""D:\data\"; --api-key ATTACKER_KEY \"pwn""#),
                "models_dir 必须被完整引号包裹：got {}", expanded);
    }
}
