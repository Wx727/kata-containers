use crate::HypervisorConfig;
use crate::shyper::sl;
use crate::hypervisor_persist::HypervisorState;
use crate::HYPERVISOR_SHYPER;
use crate::device::DeviceType;

use anyhow::{anyhow, Context, Result};
use std::process::{Child, Command, Stdio};
use std::fs::File;
use std::io::{Seek, SeekFrom};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use std::sync::Arc;
use serde_json::Value;

const DISK_POOL_SIZE: usize = 2;
const DUMMY_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024; // 10GB

pub struct ShyperInner {
    vm_process: Arc<Mutex<Option<Child>>>,
    exit_notify: Sender<()>,
    config: HypervisorConfig,
    pending_devices: Vec<DeviceType>,
    id: String,
    device_slots: [Option<String>; DISK_POOL_SIZE],
    vsock_cid: u32,
}

impl std::fmt::Debug for ShyperInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShyperInner")
            .field("config", &self.config)
            .field("id", &self.id)
            .finish()
    }
}

impl ShyperInner {
    pub fn new(exit_notify: Sender<()>) -> Self {
        Self {
            vm_process: Arc::new(Mutex::new(None)),
            exit_notify,
            config: HypervisorConfig::default(),
            pending_devices: Vec::new(),
            id: String::default(),
            device_slots: Default::default(),
            vsock_cid: 3, // Default CID
        }
    }

    pub fn set_hypervisor_config(&mut self, config: HypervisorConfig) {
        self.config = config;
    }
    
    // 核心：准备虚拟机，只打印日志
    pub async fn prepare_vm(&mut self, id: &str, _netns: Option<String>) -> Result<()> {
        info!(sl(), "prepare_vm called for GVM {}", id);
        self.id = id.to_string();
        Ok(())
    }

    pub async fn add_device(&mut self, device: DeviceType) -> Result<()> {
        let vm_running = { 
            let vm_p = self.vm_process.lock().await;
            vm_p.is_some() 
        };

        if !vm_running {
            info!(sl(), "Adding device to pending list (VM not running): {:?}", device);
            if let DeviceType::Vsock(ref vsock) = device {
                 if vsock.config.guest_cid != u32::MAX {
                     info!(sl(), "Updating Vsock CID to {}", vsock.config.guest_cid);
                     self.vsock_cid = vsock.config.guest_cid;
                 }
            }
            self.pending_devices.push(device);
            return Ok(());
        }

        // VM is running, perform hot-replacement
        info!(sl(), "Hot-replacing device (VM running): {:?}", device);
        if let DeviceType::Block(blk) = device {
             let path = blk.config.path_on_host;
             let _index = blk.config.index as usize;
             
             // In our scheme:
             // index 0 -> rootfs (usually not hotplugged this way)
             // index from Kata usually 1, 2, ...
             // We map Kata index to our configuration index.
             // If we pre-allocated slots in start_vm, we should have a mapping.
             // For simplicity, let's assume direct mapping if possible,
             // BUT we need to match the configuration ID used in generation.
             
             // In start_vm generation logic below:
             // cfg_list: [ i + 1, size ]
             // mediate list index = i + 1.
             // So if we have 8 slots (idx 0..7), mediate indices are 1..8.
             // 
             // Ideally we find a free slot. But Kata usually assigns the index in BlockConfig?
             // If Kata assigns index, we must respect it or map it.
             // Let's assume Kata's index corresponds to the 'device index' which we mapped to 'mediate index'.
             
             // NOTE: Kata's index usually starts from 0 or 1.
             // If we have a pool, we need to know WHICH pool slot to use.
             // If Kata says "index=1", does it mean "vdb"?
             
             // Simplification: We blindly try to replace using the index provided by Kata as the target configuration ID?
             // Wait, the command is `replace-disk 1 <ConfigID> <Path>`.
             // In `kata.json`, we generated devices.
             // Let's ensure the generated `config_id` matches what we expect here.
             
             // We'll use `pending_devices` length at start to determine filled slots, 
             // but `add_device` at runtime implies a NEW device.
             // We need to find an available slot in our `device_slots`.
             
             let mut slot_idx = None;
             
             // Try to use the index provided by Kata if possible?
             // Or just find first empty slot.
             // But if we return success, Kata assumes device attached.
             // The Guest OS will see a device update.
             
             for (i, slot) in self.device_slots.iter_mut().enumerate() {
                 if slot.is_none() {
                     slot_idx = Some(i);
                     *slot = Some(path.clone());
                     break;
                 }
                 // If we find the same path, maybe it's an update?
             }
             
             let slot_idx = slot_idx.ok_or_else(|| anyhow!("No free disk slots in Shyper pool"))?;
             
             // The ID in kata.json was generated as: idx + 1 (where idx is loop 0..POOL_SIZE)
             // Wait, in start_vm (logic to be written), 
             // mediate list: 0=rootfs, 1=slot0, 2=slot1 ...
             // cfg_list for slot i: [ i+1, size ]
             // So Config ID = i + 1.
             
             let config_id = slot_idx + 1; 
             
             let work_dir = "/root/taishan200-boot/gvm";
             
             // Debug: check device size before hotplug
             if let Ok(meta) = std::fs::metadata(&path) {
                 info!(sl(), "Hotplug device host metadata: len={}", meta.len());
             } else {
                 // Try blockdev size
                 let _ = Command::new("blockdev")
                     .arg("--getsize64")
                     .arg(&path)
                     .output()
                     .map(|o| {
                         let s = String::from_utf8_lossy(&o.stdout);
                         info!(sl(), "Hotplug device blockdev size: {}", s.trim());
                     });
             }

             info!(sl(), "Replacing disk at slot {} (ConfigID {}) with {}", slot_idx, config_id, path);
             
             let status = Command::new("./shyper")
                .current_dir(work_dir)
                .arg("vm")
                .arg("replace-disk")
                .arg("1") // VM ID
                .arg(config_id.to_string())
                .arg(&path)
                .status()
                .context("failed to replace disk")?;
                
             if !status.success() {
                 return Err(anyhow!("replace-disk failed with status {:?}", status));
             }
             
             return Ok(());
        }

        warn!(sl(), "Unsupported device type for hotplug: {:?}", device);
        Ok(())
    }

    // 核心：启动虚拟机
    pub async fn start_vm(&mut self, _timeout: i32) -> Result<()> {
        info!(sl(), "start_vm called, preparing configs");

        let work_dir = "/root/taishan200-boot/gvm";
        let mediate_filename = format!("mediate_gen_{}.json", self.id);
        let kata_filename = format!("kata_gen_{}.json", self.id);
        let mediate_path = format!("{}/{}", work_dir, mediate_filename);
        let kata_path = format!("{}/{}", work_dir, kata_filename);

        // 1. Generate mediate.json
        // Base image is hardcoded as in the original mediate.json, BUT if we have pending devices,
        // we check if the first one is the rootfs (based on user request "first disk is VM rootfs").
        let mut mediate_list = Vec::new();
        let mut blk_devices = Vec::new();
        
        let mut pending_blocks = Vec::new();
        for dev in &self.pending_devices {
             if let DeviceType::Block(blk) = dev {
                 pending_blocks.push(blk.config.path_on_host.clone());
             }
        }

        // Handle RootFS (Index 0 in mediate.json)
        if !pending_blocks.is_empty() {
             // User provided rootfs via add_device
             mediate_list.push(pending_blocks[0].clone());
        } else {
             // Default implicit rootfs
             mediate_list.push("kata-containers-nopart.img".to_string());
        }

        // Fill slots for pool (Indices 1..DISK_POOL_SIZE)
        // If pending_blocks had item 0, we used it for rootfs.
        // Remaining pending blocks go to pool slots.
        let start_idx = if !pending_blocks.is_empty() { 1 } else { 0 };

        for i in 0..DISK_POOL_SIZE {
            if start_idx + i < pending_blocks.len() {
                // Use pending block
                let path = pending_blocks[start_idx + i].clone();
                match self.device_slots[i] {
                    None => self.device_slots[i] = Some(path.clone()),
                    Some(_) => {} // Already set? Should match.
                }
                
                mediate_list.push(path.clone());
                blk_devices.push(Some(path));
            } else {
                // Create dummy file
                let dummy_path = format!("{}/dummy_disk_{}.img", work_dir, i);
                let f = std::fs::OpenOptions::new().write(true).create(true).open(&dummy_path);
                if let Ok(f) = f {
                    if let Err(e) = f.set_len(DUMMY_FILE_SIZE) {
                         warn!(sl(), "Failed to set dummy file len: {}", e);
                    }
                }

                let _ = Command::new("mkfs.ext4")
                    .arg("-F")
                    .arg(&dummy_path)
                    .output()
                    .map_err(|e| warn!(sl(), "Failed to format dummy file: {}", e));
                
                mediate_list.push(dummy_path.clone());
                blk_devices.push(Some(dummy_path));
                
                // Ensure slot is marked as dummy if not set
                if self.device_slots[i].is_none() {
                     // We don't save dummy paths to device_slots usually, as they are not "devices".
                     // But for hotplug, we need to know it's empty.
                }
            }
        }
        
        let mediate_json = serde_json::json!({
            "mediated": mediate_list
        });
        
        let f = File::create(&mediate_path).context(format!("create mediate json {}", mediate_path))?;
        serde_json::to_writer(f, &mediate_json).context("write mediate json")?;
        
        // 2. Generate kata.json
        let template_path = format!("{}/kata.json", work_dir);
        let f = File::open(&template_path).context(format!("open kata.json template {}", template_path))?;
        let mut kata_config: Value = serde_json::from_reader(f).context("parse kata.json")?;
        
        // Fix Rootfs Kernel Command Line if needed
        // If the user provided a partitioned image (standard Kata image), we likely need root=/dev/vda1
        // If it's a raw FS image, root=/dev/vda is fine.
        // We use a heuristic: check if `file` says "boot sector" or "partition"
        if !pending_blocks.is_empty() {
            let rootfs_path = &pending_blocks[0];
            let is_partitioned = Command::new("file")
                .arg(rootfs_path)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
                .map(|s| s.contains("partition") || s.contains("boot sector") || s.contains("mbr"))
                .unwrap_or(false);

            if is_partitioned {
                info!(sl(), "Detected partitioned rootfs image, adjusting kernel cmdline to root=/dev/vda1");
                if let Some(cmdline) = kata_config.get_mut("cmdline").and_then(|v| v.as_str()) {
                    let new_cmdline = cmdline.replace("root=/dev/vda", "root=/dev/vda1");
                    kata_config["cmdline"] = serde_json::Value::String(new_cmdline);
                }
            }
        }

        // Add devices
        if let Some(emu_list) = kata_config.pointer_mut("/emulated_device/emulated_device_list").and_then(|v| v.as_array_mut()) {
             // Update Vsock CID
             for dev in emu_list.iter_mut() {
                 if dev["type"] == "EMU_DEVICE_T_VIRTIO_VSOCK" {
                     info!(sl(), "Updating Vsock config with CID {}", self.vsock_cid);
                     dev["cfg_list"] = serde_json::json!([self.vsock_cid]);
                 }
                 // Fix Rootfs Size (virtio_blk with cfg_list[0] == 0)
                 if dev["type"] == "EMU_DEVICE_T_VIRTIO_BLK_MEDIATED" {
                     if let Some(cfg_list) = dev.get_mut("cfg_list").and_then(|v| v.as_array_mut()) {
                         if cfg_list.len() > 0 && cfg_list[0] == 0 {
                             // This is the rootfs device
                             let rootfs_path = if !pending_blocks.is_empty() {
                                 &pending_blocks[0]
                             } else {
                                 "kata-containers-nopart.img" // Actually resolved relative to cwd?
                                 // But we need size.
                             };
                             
                             // Try to get size
                             let mut size = 0;
                             if let Ok(mut f) = File::open(rootfs_path) {
                                 if let Ok(s) = f.seek(SeekFrom::End(0)) {
                                     size = s;
                                 }
                             }
                             if size == 0 {
                                 // Fallback for relative path or missing file, assume 4GB default or try to find it
                                 // Usually passed as absolute path.
                                 // If "kata-containers-nopart.img", it might be in work_dir.
                                 let p = format!("{}/{}", work_dir, rootfs_path);
                                 if let Ok(mut f) = File::open(&p) {
                                      if let Ok(s) = f.seek(SeekFrom::End(0)) {
                                          size = s;
                                      }
                                 }
                             }

                             if size > 0 {
                                 info!(sl(), "Updating rootfs (Device 0) size to {}", size);
                                 // cfg_list[0] is start sector (0), cfg_list[1] is end sector (size / 512 + start)
                                 let sectors = size / 512;
                                 cfg_list[1] = serde_json::json!(sectors);
                             }
                         }
                     }
                 }
             }

             // Base IPA and IRQ for new devices (Start after existing ones)
             let mut base_ipa = 0x60007000; 
             let mut base_irq = 502;

             // Note: Rootfs device is assumed to be in the template (Index 0).
             // We only add pool devices here.
             
             // We iterate 0..POOL_SIZE
             for i in 0..DISK_POOL_SIZE {
                let path = blk_devices[i].as_ref().unwrap(); // Should always be some
                
                // Get block device size via seek
                let size = File::open(path).and_then(|mut f| f.seek(SeekFrom::End(0))).unwrap_or(DUMMY_FILE_SIZE);
                let sectors = size / 512;

                let new_dev = serde_json::json!({
                    "name": format!("virtio_blk_pool_{}@{:x}", i, base_ipa),
                    "base_ipa": format!("0x{:x}", base_ipa),
                    "length": "0x1000",
                    "irq_id": base_irq,
                    "cfg_num": 2,
                    "cfg_list": [
                        0, // Start Sector
                        sectors // End Sector (Size in sectors)
                    ],
                    "type": "EMU_DEVICE_T_VIRTIO_BLK_MEDIATED"
                });
                
                emu_list.push(new_dev);
                
                base_ipa += 0x1000;
                base_irq += 1;
             }
        }
        
        let f = File::create(&kata_path).context("create kata json")?;
        serde_json::to_writer(f, &kata_config).context("write kata json")?;

        info!(sl(), "Configs generated: {} {}", mediate_path, kata_path);

        // Lock vm_process
        let mut vm_process = self.vm_process.lock().await;

        // 3. Start Daemon
        let mut daemon_cmd = Command::new("./shyper");
        daemon_cmd.current_dir(work_dir);
        daemon_cmd.arg("system").arg("daemon").arg(&mediate_filename);
        
        // Redirect logs
        let stdout_file = File::create(format!("/tmp/shyper_vm_{}.stdout", self.id)).ok();
        let stderr_file = File::create(format!("/tmp/shyper_vm_{}.stderr", self.id)).ok();

        if let Some(f) = stdout_file { daemon_cmd.stdout(Stdio::from(f)); }
        if let Some(f) = stderr_file { daemon_cmd.stderr(Stdio::from(f)); }

        info!(sl(), "Starting daemon2: {:?}", daemon_cmd);
        let mut child = daemon_cmd.spawn().context("spawn shyper daemon")?;
        
        info!(sl(), "Daemon spawned with PID {}", child.id());
        
        // wait a moment to ensure daemon starts
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Check if daemon is still alive
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = stderr_file; // Keep explicit reference if needed, but file handles are closed on drop?
                                     // Actually capturing stderr content would be better but it's redirected to file.
                return Err(anyhow!("Shyper daemon exited prematurely with status: {}", status));
            }
            Ok(None) => {
                 info!(sl(), "Daemon is running (PID {}). Proceeding to config.", child.id());
            }
            Err(e) => {
                 warn!(sl(), "Failed to check daemon status: {}", e);
            }
        }

        // 4. Config VM
        let mut config_cmd = Command::new("./shyper");
        config_cmd.current_dir(work_dir);
        config_cmd.arg("vm").arg("config").arg(&kata_filename);
        
        info!(sl(), "Configuring VM: {:?}", config_cmd);
        
        // Capture output of config command
        let config_output = config_cmd.output().context("run vm config")?;
        
        if !config_output.status.success() {
             let _ = child.kill();
             let stdout_s = String::from_utf8_lossy(&config_output.stdout);
             let stderr_s = String::from_utf8_lossy(&config_output.stderr);
             warn!(sl(), "VM config failed. Stdout: {}, Stderr: {}", stdout_s, stderr_s);
             return Err(anyhow!("vm config failed: {}", stderr_s));
        } else {
             info!(sl(), "VM configured successfully");
        }
        
        // 5. Boot VM
        // Note: Using 'wait' on daemon later, so boot command returns?
        // In the original script: ./shyper vm boot 1; wait $DAEMON_PID
        let mut boot_cmd = Command::new("./shyper");
        boot_cmd.current_dir(work_dir);
        boot_cmd.arg("vm").arg("boot").arg("1"); 
        
        info!(sl(), "Booting VM: {:?}", boot_cmd);
        let boot_output = boot_cmd.output().context("run vm boot")?;
        if !boot_output.status.success() {
            let _ = child.kill();
            let stdout_s = String::from_utf8_lossy(&boot_output.stdout);
            let stderr_s = String::from_utf8_lossy(&boot_output.stderr);
            warn!(sl(), "VM boot failed. Stdout: {}, Stderr: {}", stdout_s, stderr_s);
            return Err(anyhow!("vm boot failed: {}", stderr_s));
        } else {
            info!(sl(), "VM booted command sent successfully");
        }
        
        // 6. Setup waiter
        let exit_notify_clone = self.exit_notify.clone();
        let vm_process_arc_clone = self.vm_process.clone();
        
        // We track the daemon process as the main process
        *vm_process = Some(child);
        
        tokio::task::spawn_blocking(move || {
            loop {
                 {
                    let mut vm_proc_guard = vm_process_arc_clone.blocking_lock();
                    if let Some(child) = vm_proc_guard.as_mut() {
                        match child.try_wait() {
                            Ok(Some(_)) => {
                                let _ = exit_notify_clone.blocking_send(());
                                break;
                            }
                            Ok(None) => {}
                            Err(_) => {
                                let _ = exit_notify_clone.blocking_send(());
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                 }
                 std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
        
        info!(sl(), "Shyper GVM started successfully");
        Ok(())
    }

    // 核心：停止虚拟机
    pub async fn stop_vm(&mut self) -> Result<()> {
        info!(sl(), "stop_vm called, destroying GVM");
        
        let work_dir = "/root/taishan200-boot/gvm";
        // 1. Destroy VM
        let status = Command::new("./shyper")
            .current_dir(work_dir)
            .arg("vm")
            .arg("destroy")
            .arg("1")
            .status()
            .context("failed to destroy GVM via shyper")?;

        if !status.success() {
            warn!(sl(), "destroy GVM command exited with {:?}", status);
        }

        // 2. Kill Daemon
        let mut vm_process = self.vm_process.lock().await;
        if let Some(child) = vm_process.as_mut() {
            info!(sl(), "Killing shyper daemon");
            let _ = child.kill();
            let _ = child.wait();
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
        Ok(format!("vsock://{}:1024", self.vsock_cid))
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

    pub async fn get_ns_path(&self) -> Result<String> {
        Ok(format!(
            "/proc/{}/task/{}/ns",
            std::process::id(),
            std::process::id()
        ))
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

    pub async fn save(&self) -> Result<HypervisorState> {
        info!(sl(), "Saving Shyper hypervisor state: {:?}", self.config);
        Ok(HypervisorState {
            hypervisor_type: HYPERVISOR_SHYPER.to_string(),
            config: self.config.clone(),
            ..Default::default()
        })
    }

    pub async fn restore(exit_notify: Sender<()>, state: HypervisorState) -> Result<Self> {
        Ok(Self {
            vm_process: Arc::new(Mutex::new(None)),
            exit_notify,
            config: state.config,
            pending_devices: Vec::new(),
            id: String::default(),
            device_slots: Default::default(),
            vsock_cid: 3,
        })
    }
}