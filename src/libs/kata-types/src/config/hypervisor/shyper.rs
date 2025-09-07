use std::io::Result;
use std::path::Path;
use std::sync::Arc;

use super::register_hypervisor_plugin;

use crate::config::{ConfigPlugin, TomlConfig};
use crate::{eother, validate_path};

// 你的可执行文件的默认路径
const DEFAULT_SHYPER_BINARY_PATH: &str = "/usr/bin/shyper-cli";
const DEFAULT_SHYPER_GUEST_KERNEL_IMAGE: &str = "/usr/share/kata-containers/vmlinuz-shyper";

/// Hypervisor name for shyper, used to index `TomlConfig::hypervisor`.
pub const HYPERVISOR_NAME_SHYPER: &str = "shyper";

/// Configuration information for shyper.
#[derive(Default, Debug, Deserialize, Serialize, Clone)]
pub struct ShyperConfig {}

impl ShyperConfig {
    /// Create a new instance of `ShyperConfig`.
    pub fn new() -> Self {
        ShyperConfig {}
    }

    /// Register the shyper plugin.
    pub fn register(self) {
        let plugin = Arc::new(self);
        register_hypervisor_plugin(HYPERVISOR_NAME_SHYPER, plugin);
    }
}

impl ConfigPlugin for ShyperConfig {
    fn get_max_cpus(&self) -> u32 {
        0
    }

    fn get_min_memory(&self) -> u32 {
        0
    }
    
    fn name(&self) -> &str {
        HYPERVISOR_NAME_SHYPER
    }

    // 设置默认值
    fn adjust_config(&self, conf: &mut TomlConfig) -> Result<()> {
        if let Some(shyper) = conf.hypervisor.get_mut(HYPERVISOR_NAME_SHYPER) {
            // 如果路径为空，则设置默认值
            if shyper.path.is_empty() {
                shyper.path = DEFAULT_SHYPER_BINARY_PATH.to_string();
            }
            // 设置内核镜像的默认路径
            if shyper.boot_info.kernel.is_empty() {
                shyper.boot_info.kernel = DEFAULT_SHYPER_GUEST_KERNEL_IMAGE.to_string();
            }
        }
        Ok(())
    }

    // 验证配置
    fn validate(&self, conf: &TomlConfig) -> Result<()> {
        if let Some(shyper) = conf.hypervisor.get(HYPERVISOR_NAME_SHYPER) {
            // 验证路径是否为空
            if shyper.path.is_empty() {
                return Err(eother!("Shyper path is empty"));
            }
            // 验证路径是否存在
            validate_path!(shyper.path, "Shyper binary path `{}` is invalid: {}")?;

            // 验证内核镜像路径
            if shyper.boot_info.kernel.is_empty() {
                return Err(eother!("Guest kernel image for shyper is empty"));
            }
            validate_path!(shyper.boot_info.kernel, "Shyper kernel path `{}` is invalid: {}")?;
        }
        Ok(())
    }
}