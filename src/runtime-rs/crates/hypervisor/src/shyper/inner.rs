use crate::HypervisorConfig;
use crate::shyper::sl;

use anyhow::{anyhow, Context, Result};
use std::process::{Child, Command};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use std::sync::Arc;

pub struct ShyperInner {
    vm_process: Arc<Mutex<Option<Child>>>,
    #[allow(dead_code)]
    exit_notify: Sender<()>,
    config: HypervisorConfig,
    sandbox_id: Arc<Mutex<String>>,
}

impl std::fmt::Debug for ShyperInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShyperInner")
            .field("config", &self.config)
            .finish()
    }
}

impl ShyperInner {
    pub fn new(exit_notify: Sender<()>) -> Self {
        Self {
            vm_process: Arc::new(Mutex::new(None)),
            exit_notify,
            config: HypervisorConfig::default(),
            sandbox_id: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn set_hypervisor_config(&mut self, config: HypervisorConfig) {
        self.config = config;
    }
    
    // 核心：准备虚拟机，只打印日志
    pub async fn prepare_vm(&mut self, id: &str, _netns: Option<String>) -> Result<()> {
        info!(sl(), "prepare_vm called for GVM with id: {}", id);
        let mut sandbox_id = self.sandbox_id.lock().await;
        *sandbox_id = id.to_string();
        Ok(())
    }

    // 核心：启动虚拟机
    pub async fn start_vm(&mut self, _timeout: i32) -> Result<()> {
        let work_dir = "/root/taishan200-boot/gvm";
        let shyper_binary = "./shyper";
        let vm_config = "kata.json";
        let vm_id = "1";

        // 配置 VM（使用 output() 确保命令执行完成，并可以获取输出）
        info!(sl(), "Configuring VM with config {}", vm_config);
        let output = Command::new(shyper_binary)
            .arg("vm")
            .arg("config")
            .arg(vm_config)
            .current_dir(work_dir)
            .output()
            .context("failed to configure VM")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("VM configuration failed: {}", stderr));
        }

        // 启动 VM（使用 output() 确保命令执行完成，并可以获取输出）
        info!(sl(), "Booting VM {}", vm_id);
        let output = Command::new(shyper_binary)
            .arg("vm")
            .arg("boot")
            .arg(vm_id)
            .current_dir(work_dir)
            .output()
            .context("failed to boot VM")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("VM boot failed: {}", stderr));
        }

        let mut vm_process = self.vm_process.lock().await;
        *vm_process = None;
        
        Ok(())
    }

    // 核心：停止虚拟机
    pub async fn stop_vm(&mut self) -> Result<()> {
        info!(sl(), "stop_vm called, destroying GVM");

        // 这里直接调用 CLI 工具销毁 VM
        let status = Command::new("/root/shyper_rk3588")
            .arg("vm")
            .arg("destroy")
            .arg("1")
            .status()
            .context("failed to destroy GVM via shyper_rk3588")?;

        if !status.success() {
            warn!(sl(), "destroy GVM command exited with {:?}", status);
        }

        Ok(())
    }

    // 核心：等待虚拟机退出
    // 注意：Type-1 Hypervisor 中，VM 不是进程，无法等待进程退出
    // 这里返回占位符值，实际应该通过其他方式（如 API 调用）检查 VM 状态
    pub async fn wait_vm(&self) -> Result<i32> {
        info!(sl(), "wait_vm called for Type-1 Hypervisor (VM is not a process)");
        // Type-1 Hypervisor 中，VM 不是进程，无法等待进程退出
        // 返回 0 作为占位符，表示 VM 由 Hypervisor 管理
        warn!(sl(), "Type-1 Hypervisor: VM is not a process, cannot wait for process exit. Returning placeholder exit code 0");
        Ok(0)
    }
    
    // 核心：获取 Agent 套接字
    // 注意：shyper 不支持 vsock，这里先返回占位符让 VM 能启动
    // 后续需要根据实际的通信方式（TCP/Unix socket等）来实现
    pub async fn get_agent_socket(&self) -> Result<String> {
        // 方案1: 使用 remote scheme + Unix socket 路径（如果 shyper 创建了 Unix socket）
        // const REMOTE_SCHEME: &str = "remote";
        // let sandbox_id = self.sandbox_id.lock().await;
        // let socket_path = format!("/tmp/shyper-agent-{}.sock", sandbox_id);
        // let socket = format!("{}://{}", REMOTE_SCHEME, socket_path);
        // info!(sl(), "get_agent_socket called, returning: {}", socket);
        // Ok(socket)
        
        // 方案2: 使用 remote scheme + TCP 地址（如果 shyper 使用 TCP）
        // 注意：agent 的 remote scheme 实际上只支持 Unix socket，不支持 TCP
        // 如果需要 TCP，需要修改 agent 代码
        
        // 方案3: 临时占位符 - 返回一个不存在的路径，让连接失败但不阻止 VM 启动
        // 这样可以让 VM 先启动起来，后续再实现真正的通信
        const REMOTE_SCHEME: &str = "remote";
        let sandbox_id = self.sandbox_id.lock().await;
        // 使用一个占位符路径，这个文件不存在，连接会失败，但不影响 VM 启动
        let placeholder_path = format!("/tmp/shyper-agent-{}.sock", sandbox_id);
        let socket = format!("{}://{}", REMOTE_SCHEME, placeholder_path);
        warn!(sl(), "get_agent_socket called, returning placeholder: {} (shyper does not support vsock, agent connection will fail but VM can start)", socket);
        Ok(socket)
    }
    
    // 核心：获取 VMM 进程 ID
    // 注意：Type-1 Hypervisor 中，VM 不是进程，返回空列表
    pub async fn get_pids(&self) -> Result<Vec<u32>> {
        // Type-1 Hypervisor: VM 不是进程，没有进程 ID
        Ok(vec![])
    }
    
    // 核心：获取 VMM 主线程 ID
    // 注意：Type-1 Hypervisor 中，VM 不是进程，返回占位符值
    // 这个值用于 OCI state 的 pid 字段，返回 0 表示 VM 由 Hypervisor 管理
    pub async fn get_vmm_master_tid(&self) -> Result<u32> {
        // Type-1 Hypervisor: VM 不是进程，返回占位符值 0
        // 这表示 VM 由 Hypervisor 直接管理，不是作为进程运行
        Ok(0)
    }

    // 核心：清理资源
    pub async fn cleanup(&self) -> Result<()> {
        info!(sl(), "cleanup called, no specific resources to clean up");
        Ok(())
    }

    // 核心：检查
    pub async fn check(&self) -> Result<()> {
        info!(sl(), "check called, returning success");
        Ok(())
    }

    // 辅助方法
    pub fn hypervisor_config(&self) -> HypervisorConfig {
        self.config.clone()
    }
}