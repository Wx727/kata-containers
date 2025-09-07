mod inner;

use crate::device::DeviceType;
use crate::hypervisor_persist::HypervisorState;
use crate::{Hypervisor, MemoryConfig, VcpuThreadIds};
use crate::HypervisorConfig;
use inner::ShyperInner;
use kata_types::capabilities::{Capabilities, CapabilityBits};

use anyhow::Result;
use async_trait::async_trait;

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};

pub fn sl() -> slog::Logger {
    slog_scope::logger().new(o!("subsystem" => "shyper"))
}

#[derive(Debug)]
pub struct Shyper {
    inner: Arc<RwLock<ShyperInner>>,
    exit_waiter: Mutex<(mpsc::Receiver<()>, i32)>,
}

impl Default for Shyper {
    fn default() -> Self {
        Self::new()
    }
}

impl Shyper {
    pub fn new() -> Self {
        let (exit_notify, exit_waiter) = mpsc::channel(1);

        Self {
            inner: Arc::new(RwLock::new(ShyperInner::new(exit_notify))),
            exit_waiter: Mutex::new((exit_waiter, 0)),
        }
    }

    pub async fn set_hypervisor_config(&self, config: HypervisorConfig) {
        let mut inner = self.inner.write().await;
        inner.set_hypervisor_config(config)
    }
}

#[async_trait]
impl Hypervisor for Shyper {
    // 核心：准备虚拟机
    async fn prepare_vm(
        &self,
        id: &str,
        netns: Option<String>,
        _annotations: &HashMap<String, String>,
    ) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.prepare_vm(id, netns).await
    }

    // 核心：启动虚拟机
    async fn start_vm(&self, timeout: i32) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.start_vm(timeout).await
    }

    // 核心：等待虚拟机退出
    async fn wait_vm(&self) -> Result<i32> {
        info!(sl(), "wait shyper vm");
        let mut waiter = self.exit_waiter.lock().await;
        waiter.0.recv().await;
        let inner = self.inner.read().await;
        if let Ok(exit_code) = inner.wait_vm().await {
            waiter.1 = exit_code;
        }
        Ok(waiter.1)
    }

    // 核心：停止虚拟机
    async fn stop_vm(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.stop_vm().await
    }

    // 核心：获取 Agent 套接字（实现了优雅的 fallback）
    async fn get_agent_socket(&self) -> Result<String> {
        // 1) 先尝试让 inner 返回（如果实现了的话）
        {
            let inner = self.inner.read().await;
            match inner.get_agent_socket().await {
                Ok(s) if !s.is_empty() => return Ok(s),
                _ => {
                    // 继续尝试 fallback
                }
            }
        }

        // 2) 尝试读取环境变量（便于调试/运行时覆盖）
        if let Ok(env_sock) = env::var("KATA_AGENT_SOCKET") {
            if !env_sock.is_empty() {
                return Ok(env_sock);
            }
        }

        // 3) 最后返回一个合理的默认值（unix socket 路径优先）
        //    这个默认值与你的 guest-agent 配置要一致；若使用 TCP/virtio-net，可用 "tcp://127.0.0.1:10000"
        //    你可以根据实际需要替换为 tcp 地址（例如 "127.0.0.1:10000"）或 unix 路径。
        //    这里采用 unix socket 的默认路径（常见于 kata 安装）：/run/kata-containers/agent.sock
        Ok(String::from("unix:///run/kata-containers/agent.sock"))
    }

    // 核心：获取 VMM 进程 ID
    async fn get_pids(&self) -> Result<Vec<u32>> {
        let inner = self.inner.read().await;
        inner.get_pids().await
    }

    // 核心：获取 VMM 主线程 ID
    async fn get_vmm_master_tid(&self) -> Result<u32> {
        let inner = self.inner.read().await;
        inner.get_vmm_master_tid().await
    }

    // 核心：清理资源
    async fn cleanup(&self) -> Result<()> {
        let inner = self.inner.read().await;
        inner.cleanup().await
    }

    // -----------------------------------------------------------
    // 以下功能先返回默认值，保证编译通过
    // -----------------------------------------------------------

    async fn pause_vm(&self) -> Result<()> {
        Ok(())
    }

    async fn save_vm(&self) -> Result<()> {
        Ok(())
    }

    async fn resume_vm(&self) -> Result<()> {
        Ok(())
    }

    async fn resize_vcpu(&self, old_vcpus: u32, new_vcpus: u32) -> Result<(u32, u32)> {
        Ok((old_vcpus, new_vcpus))
    }

    async fn resize_memory(&self, new_mem_mb: u32) -> Result<(u32, MemoryConfig)> {
        Ok((new_mem_mb, MemoryConfig::default()))
    }

    async fn add_device(&self, device: DeviceType) -> Result<DeviceType> {
        Ok(device)
    }

    async fn remove_device(&self, _device: DeviceType) -> Result<()> {
        Ok(())
    }

    async fn update_device(&self, _device: DeviceType) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&self) {}

    async fn hypervisor_config(&self) -> HypervisorConfig {
        let inner = self.inner.read().await;
        inner.hypervisor_config()
    }

    async fn get_thread_ids(&self) -> Result<VcpuThreadIds> {
        Ok(VcpuThreadIds::default())
    }

    async fn get_ns_path(&self) -> Result<String> {
        Ok(String::new())
    }

    async fn check(&self) -> Result<()> {
        let inner = self.inner.read().await;
        inner.check().await
    }

    async fn get_jailer_root(&self) -> Result<String> {
        Ok(String::new())
    }

    async fn save_state(&self) -> Result<HypervisorState> {
        Ok(HypervisorState::default())
    }

    async fn capabilities(&self) -> Result<Capabilities> {
        Ok(Capabilities::default())
    }

    async fn get_hypervisor_metrics(&self) -> Result<String> {
        Ok(String::new())
    }

    async fn set_capabilities(&self, _flag: CapabilityBits) {}

    async fn set_guest_memory_block_size(&self, _size: u32) {}

    async fn guest_memory_block_size(&self) -> u32 {
        0
    }

    async fn get_passfd_listener_addr(&self) -> Result<(String, u32)> {
        Ok((String::new(), 0))
    }
}
