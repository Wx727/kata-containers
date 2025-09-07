use crate::HypervisorConfig;
use crate::shyper::sl;

use anyhow::{anyhow, Context, Result};
use std::process::{Child, Command};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use std::sync::Arc;

pub struct ShyperInner {
    vm_process: Arc<Mutex<Option<Child>>>,
    exit_notify: Sender<()>,
    config: HypervisorConfig,
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
        }
    }

    pub fn set_hypervisor_config(&mut self, config: HypervisorConfig) {
        self.config = config;
    }
    
    // 核心：准备虚拟机，只打印日志
    pub async fn prepare_vm(&mut self, _id: &str, _netns: Option<String>) -> Result<()> {
        info!(sl(), "prepare_vm called for GVM");
        Ok(())
    }

    // 核心：启动虚拟机
    pub async fn start_vm(&mut self, _timeout: i32) -> Result<()> {
        info!(sl(), "start_vm called, launching boot script for GVM");

        // 锁住 vm_process guard（和之前的模式一致）
        let mut vm_process = self.vm_process.lock().await;

        // 使用绝对路径启动你的脚本（确保脚本可执行）
        let mut cmd = Command::new("/root/boot_gvm1_new.sh");
        // 如果脚本里面使用了相对路径（例如 ./shyper_rk3588），设置工作目录
        cmd.current_dir("/root");

        info!(sl(), "Attempting to run boot script: {:?}", cmd);

        // spawn 子进程（非阻塞）
        let child = cmd.spawn().context("failed to start Rust-Shyper GVM script")?;

        // 克隆用于等待和通知的句柄
        let exit_notify_clone = self.exit_notify.clone();
        let vm_process_arc_clone = self.vm_process.clone();

        // 后台等待子进程退出，并在退出时通知
        tokio::task::spawn_blocking(move || {
            // blocking_lock 会阻塞直到能获取 Mutex
            let mut vm_proc_guard = vm_process_arc_clone.blocking_lock();
            if let Some(child) = vm_proc_guard.as_mut() {
                // 等待子进程结束（blocking）
                let _ = child.wait();
                // 发送退出通知（blocking_send 用于在 blocking context send）
                let _ = exit_notify_clone.blocking_send(());
            }
        });

        // 将 child 存入 struct，以便 stop_vm() / wait_vm() 使用
        *vm_process = Some(child);

        info!(sl(), "Boot script launched, background waiter installed");
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
    pub async fn wait_vm(&self) -> Result<i32> {
        info!(sl(), "Waiting for GVM process to exit");
        let mut vm_process = self.vm_process.lock().await;
        // 注意: 这里使用 as_mut() 是正确的，因为 child.wait() 需要可变引用
        let child = vm_process.as_mut().context("no VM process to wait for")?;
        
        let exit_status = child.wait().context("failed to wait for vm process")?;
        
        info!(sl(), "GVM process exited with status: {:?}", exit_status.code());
        Ok(exit_status.code().unwrap_or(0))
    }
    
    // 核心：获取 Agent 套接字（返回错误）
    pub async fn get_agent_socket(&self) -> Result<String> {
        info!(sl(), "get_agent_socket called, returning error as planned");
        Err(anyhow!("get_agent_socket is not supported"))
    }
    
    // 核心：获取 VMM 进程 ID
    pub async fn get_pids(&self) -> Result<Vec<u32>> {
        let vm_process = self.vm_process.lock().await;
        if let Some(child) = vm_process.as_ref() {
            Ok(vec![child.id()])
        } else {
            Ok(vec![])
        }
    }
    
    // 核心：获取 VMM 主线程 ID
    pub async fn get_vmm_master_tid(&self) -> Result<u32> {
        let vm_process = self.vm_process.lock().await;
        if let Some(child) = vm_process.as_ref() {
            Ok(child.id())
        } else {
            Err(anyhow!("VM process not found"))
        }
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