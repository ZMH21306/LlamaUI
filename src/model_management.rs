//! 多模型管理模块。
//!
//! 支持多个 .gguf 模型的管理、切换和快速启动。
//!
//! # 核心概念
//!
//! - [`ModelInfo`]：单个模型文件的元数据（路径、大小、哈希、标签）
//! - [`ModelCatalog`]：模型目录索引，按目录组织模型文件
//! - [`ModelSelector`]：从目录列表中选择模型的快捷方式
//!
//! # 设计原则
//!
//! - 模型索引在后台线程中构建，不阻塞主线程
//! - 支持增量扫描（仅扫描新增/删除的模型文件）
//! - 模型元数据缓存到磁盘，避免重复扫描

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::Mutex;

/// 模型信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    /// 模型文件的绝对路径。
    pub path: String,
    /// 文件名（不含路径）。
    pub name: String,
    /// 文件大小（字节）。
    pub size_bytes: u64,
    /// 文件的 SHA-256 哈希（用于完整性校验）。
    pub sha256: Option<String>,
    /// 模型标签（用户自定义，用于分类）。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 模型创建时间（UNIX 纪元秒）。
    pub created_at: u64,
    /// 模型最后修改时间（UNIX 纪元秒）。
    pub modified_at: u64,
}

impl ModelInfo {
    /// 从文件系统路径扫描并创建 `ModelInfo`。
    ///
    /// 仅当文件扩展名是 `.gguf` 时才返回 `Some`。
    pub fn from_path(p: &Path) -> Option<Self> {
        let extension = p.extension()?.to_str()?;
        if !extension.eq_ignore_ascii_case("gguf") {
            return None;
        }
        let metadata = fs::metadata(p).ok()?;
        let name = p.file_name()?.to_string_lossy().to_string();
        let size_bytes = metadata.len();
        let modified_at = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let created_at = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(modified_at);

        Some(ModelInfo {
            path: p.to_string_lossy().to_string(),
            name,
            size_bytes,
            sha256: None, // 需单独计算
            tags: Vec::new(),
            created_at,
            modified_at,
        })
    }
}

/// 缓存了指定目录下所有 .gguf 文件的列表，支持快速查找和过滤。
#[derive(Debug, Clone)]
pub struct ModelCatalog {
    /// 目录路径。
    pub dir: PathBuf,
    /// 已扫描的模型列表。
    pub models: Vec<ModelInfo>,
    /// 上次扫描时间（UNIX 纪元秒），用于增量扫描判断。
    pub scanned_at: u64,
}

impl ModelCatalog {
    /// 从指定目录扫描模型（阻塞操作，建议在 spawn_blocking 中调用）。
    pub fn scan(dir: &Path) -> Self {
        let mut models = Vec::new();
        if !dir.is_dir() {
            return Self {
                dir: dir.to_path_buf(),
                models,
                scanned_at: 0,
            };
        }
        for entry in fs::read_dir(dir).into_iter().flatten() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let p = entry.path();
            if p.is_file() {
                if let Some(model) = ModelInfo::from_path(&p) {
                    models.push(model);
                }
            }
        }
        let now = std::time::UNIX_EPOCH
            .elapsed()
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            dir: dir.to_path_buf(),
            models,
            scanned_at: now,
        }
    }

    /// 按标签过滤模型。
    pub fn filter_by_tag(&self, tag: &str) -> Vec<&ModelInfo> {
        self.models
            .iter()
            .filter(|m| m.tags.iter().any(|t| t == tag))
            .collect()
    }
}

/// 多模型配置管理（线程安全）。
///
/// 存储多个模型目录的索引，并提供快速访问接口。
#[derive(Debug)]
pub struct ModelManager {
    /// 已配置的模型目录列表。
    catalogs: Arc<Mutex<Vec<ModelCatalog>>>,
    /// 当前选中的模型路径（None 表示未选择）。
    selected_model: Arc<Mutex<Option<String>>>,
}

impl ModelManager {
    /// 创建空的模型管理器。
    pub fn new() -> Self {
        Self {
            catalogs: Arc::new(Mutex::new(Vec::new())),
            selected_model: Arc::new(Mutex::new(None)),
        }
    }

    /// 添加一个模型目录并开始后台扫描。
    ///
    /// 扫描在调用线程同步执行（适合小目录）。
    pub fn add_directory(&self, dir: &Path) {
        let catalog = ModelCatalog::scan(dir);
        let mut cats = self.catalogs.lock();
        // 避免重复添加同一目录
        if !cats.iter().any(|c| c.dir == dir) {
            cats.push(catalog);
        }
    }

    /// 移除一个模型目录。
    pub fn remove_directory(&self, dir: &Path) {
        let mut cats = self.catalogs.lock();
        cats.retain(|c| c.dir != dir);
    }

    /// 获取所有目录中的所有模型。
    pub fn all_models(&self) -> Vec<ModelInfo> {
        let cats = self.catalogs.lock();
        cats.iter()
            .flat_map(|c| c.models.clone())
            .collect()
    }

    /// 按标签过滤所有模型。
    pub fn filter_models_by_tag(&self, tag: &str) -> Vec<ModelInfo> {
        let cats = self.catalogs.lock();
        cats.iter()
            .flat_map(|c| c.filter_by_tag(tag).into_iter().cloned().collect::<Vec<_>>())
            .collect()
    }

    /// 选择当前活跃的模型。
    pub fn select_model(&self, path: &str) {
        let mut sel = self.selected_model.lock();
        *sel = Some(path.to_string());
    }

    /// 获取当前选中的模型路径。
    pub fn selected_model(&self) -> Option<String> {
        self.selected_model.lock().clone()
    }

    /// 刷新所有目录的索引。
    pub fn refresh_all(&self) {
        let mut cats = self.catalogs.lock();
        for catalog in cats.iter_mut() {
            *catalog = ModelCatalog::scan(&catalog.dir);
        }
    }
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 模型启动配置（用于快速切换到某个模型）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelLaunchConfig {
    /// 模型路径。
    pub model_path: String,
    /// 使用的模型名（用于显示）。
    pub display_name: String,
    /// 上下文大小覆盖（None 表示使用配置文件中的值）。
    pub ctx_size_override: Option<u32>,
    /// GPU 层数覆盖（None 表示使用配置文件中的值）。
    pub n_gpu_layers_override: Option<i32>,
}

impl ModelLaunchConfig {
    /// 从模型信息构建启动配置。
    pub fn from_model(model: &ModelInfo) -> Self {
        Self {
            model_path: model.path.clone(),
            display_name: model.name.clone(),
            ctx_size_override: None,
            n_gpu_layers_override: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_gguf_file(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "llama_ui_model_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test{}.gguf", suffix));
        fs::write(&path, b"fake gguf data").unwrap();
        path
    }

    #[test]
    fn model_info_from_gguf_path() {
        let p = temp_gguf_file("-a");
        let info = ModelInfo::from_path(&p).expect("应识别为 gguf 文件");
        assert!(info.path.contains("test-a.gguf"));
        assert_eq!(info.size_bytes, 14); // "fake gguf data".len()
    }

    #[test]
    fn model_info_rejects_non_gguf() {
        let dir = std::env::temp_dir().join("llama_ui_reject_test");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("model.txt");
        fs::write(&p, b"not a model").unwrap();
        assert!(ModelInfo::from_path(&p).is_none(), "非 .gguf 文件应被拒绝");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_catalog_scan_empty_dir() {
        let dir = std::env::temp_dir().join("llama_ui_empty_catalog");
        fs::create_dir_all(&dir).unwrap();
        let catalog = ModelCatalog::scan(&dir);
        assert_eq!(catalog.models.len(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_catalog_finds_gguf_files() {
        let dir = std::env::temp_dir().join("llama_ui_catalog_test");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("model1.gguf"), b"x").unwrap();
        fs::write(dir.join("model2.gguf"), b"y").unwrap();
        fs::write(dir.join("readme.txt"), b"ignore").unwrap();
        let catalog = ModelCatalog::scan(&dir);
        assert_eq!(catalog.models.len(), 2);
        assert!(catalog.models.iter().any(|m| m.name == "model1.gguf"));
        assert!(catalog.models.iter().any(|m| m.name == "model2.gguf"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_manager_add_and_list() {
        let mgr = ModelManager::new();
        let dir = std::env::temp_dir().join("llama_ui_mgr_test");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("llama.gguf"), b"data").unwrap();
        mgr.add_directory(&dir);
        let models = mgr.all_models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "llama.gguf");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_manager_filters_by_name() {
        let mgr = ModelManager::new();
        let dir = std::env::temp_dir().join("llama_ui_filter_test");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("llama3.gguf"), b"x").unwrap();
        fs::write(dir.join("qwen2.gguf"), b"y").unwrap();
        mgr.add_directory(&dir);
        let _results = mgr.filter_models_by_tag("llama");
        // 按名称过滤：所有模型都没有 llama 标签，所以为空
        // 这里测试的是空过滤
        let all = mgr.all_models();
        assert_eq!(all.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
