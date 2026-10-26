# Kata Containers 对接 Shyper 开发交接说明

## 1. 项目目标

将自研 Rust-Shyper 接入 Kata Containers，使 Kata 容器能够在 MVM 中以 GVM 形式运行。

最终目标是运行 `mini-swe-agent`。为此至少需要具备：

1. 稳定启动和停止单个 Kata 容器；
2. 容器能够访问网关、外网和 DNS；
3. 能够连续启动、停止多个容器，最终支持并发容器；
4. 支持 nerdctl/Compose 或其他 Docker 兼容调用方式；
5. 容器异常退出后能够完整回收 GVM、daemon、磁盘和网络资源。

当前只完成了单 GVM 基线和网络集成的前半部分，多容器与生命周期管理尚未完成。

## 2. 代码仓库和分支

### Kata Containers

开发机路径：

```text
/home/wx/code/kata-containers
```

服务器路径：

```text
/root/work/code/kata-containers
```

开发分支：

```text
feat-kunlun-network
```

主要适配代码：

```text
src/runtime-rs/crates/hypervisor/src/shyper/
src/runtime-rs/crates/runtimes/virt_container/src/sandbox.rs
src/runtime-rs/config/configuration-shyper.toml
```

远程仓库：

```text
GitHub: git@github.com:Wx727/kata-containers.git
Gitee:  https://gitee.com/wx727/kata-containers.git
```

服务器优先从 Gitee 拉取，避免 GitHub 网络不稳定。

关键提交：

```text
d8c7feb3b  Shyper 单容器基线
76ede593b  启动前从 Kata NetworkDevice 提取 MAC 并写入 Shyper JSON
f16b18c6d  在 Shyper 启动前执行 nerdctl CNI hook 并扫描 netns
```

`f16b18c6d` 已推送 GitHub；编写本文档时尚未成功推送 Gitee，因为开发环境没有 Gitee HTTPS 凭据。

### Rust-Shyper

开发机路径：

```text
/home/wx/code/rust-shyper-v3
```

分支：

```text
feat-kunlun-acpi-wx
```

主要网络代码：

```text
src/device/virtio/net.rs
src/device/virtio/mac.rs
```

服务器上的 Shyper 运行目录：

```text
/root/taishan200-boot/gvm
```

其中包含 `shyper`、`shyper.ko`、模板 `kata.json`、内核镜像以及运行时生成的 `kata_gen_<container-id>.json` 和 `mediate_gen_<container-id>.json`。

Kata runtime-rs 配置文件：

```text
/opt/kata/share/defaults/kata-containers/runtime-rs/configuration-shyper.toml
/opt/kata/share/defaults/kata-containers/runtime-rs/configuration.toml
```

`configuration.toml` 当前应链接到 `configuration-shyper.toml`。

注意：当前仓库中的 `tools/shyper` 和 `tools/shyper.ko` 是二进制，`mvm-daemon/src/` 为空。要修复 Shyper CLI、daemon 或内核模块生命周期问题，需要找到对应的用户态工具和内核模块源码。

## 3. 当前架构

```text
nerdctl / ctr
    -> containerd
    -> containerd-shim-kata-v2 (runtime-rs)
    -> Shyper adapter
    -> shyper userspace daemon + shyper.ko
    -> Rust-Shyper
    -> GVM Linux + kata-agent
    -> container rootfs
```

术语：

- MVM：运行 containerd、Kata shim、Shyper daemon 和 `shyper.ko` 的管理虚拟机；
- GVM：由 Shyper 创建、运行 Kata guest 内核和 kata-agent 的虚拟机；
- 容器：由 GVM 内的 kata-agent 创建，通常与 GVM 共用网络栈。

## 4. 已完成和已验证内容

### 4.1 单容器启动

已经验证：

- devmapper snapshotter 可用；
- Kata runtime-rs 能生成 Shyper VM JSON 和 mediated block JSON；
- 能启动 GVM、连接 kata-agent，并进入 BusyBox 容器；
- rootfs 使用 devmapper block device；
- vsock 通信正常；
- `nerdctl exec` 在容器未被删除时可用。

### 4.2 GVM 控制台

内核参数加入以下内容后，可以通过 `/dev/hvc0` 进入 GVM 的 kata-agent debug console：

```text
agent.debug_console agent.debug_console_vport=0
```

这个 shell 属于 GVM 根文件系统，不是 BusyBox 容器 rootfs。可通过下面命令确认：

```bash
cat /proc/1/comm
findmnt -no SOURCE,FSTYPE,OPTIONS /
```

注意：控制台存在终端状态异常，执行 `clear` 后可能无回显。调试时尽量不要使用 `clear`。

### 4.3 Shyper 网络数据面

已经手工验证：

- MVM 的 Shyper 网络接口为 `eth0`，MAC 通常为 `74:56:aa:0f:47:d0`；
- GVM 的 virtio-net MAC 可配置为 `74:56:aa:0f:47:d1` 或动态 MAC；
- MVM/GVM 配置 `172.30.0.1/24` 和 `172.30.0.2/24` 后可以互相通信；
- MVM 开启转发和 MASQUERADE 后，GVM 可以访问外网和 DNS；
- 使用预先配置好的 netns 测试时，Kata 能把 IP、路由和动态 MAC 配置进 GVM。

因此 Rust-Shyper 的单 GVM virtio-net 数据面本身可用。

### 4.4 Kata 动态 MAC 配置

`76ede593b` 已实现：

- 从 pending `DeviceType::Network` 中取得 guest MAC；
- 将 MAC 写入 Shyper JSON 的 `EMU_DEVICE_T_VIRTIO_NET.cfg_list`；
- 拒绝缺失 MAC、多 NIC 和网络热插；
- 包含相关单元测试。

### 4.5 nerdctl/CNI 启动时序修复

首次 nerdctl 测试发现：

- GVM 仍使用模板 MAC `74:56:aa:0f:47:d1`；
- GVM `eth0` 为 DOWN，没有 IP 和路由；
- nerdctl CNI hook 错误扫描到 MVM 的 `enp125s0f*`、`virbr0`、`shyper0`；
- `oci-hook.createRuntime.log` 显示 CNI 配置失败并回滚。

根因是 nerdctl 通过 `createRuntime` OCI hook 配置 CNI，而原来的 Kata 流程在 GVM 启动后才执行该 hook。Shyper daemon 又位于 MVM 宿主 netns，导致 nerdctl 根据 daemon PID 选择了错误的网络命名空间。

`f16b18c6d` 已实现：

- 只识别 nerdctl 的 `internal oci-hook createRuntime` 网络 hook；
- 在启动 Shyper GVM 前执行该 hook；
- 通过 `nerdctl/network-namespace` 明确传入 Kata 创建的 netns；
- hook 配置 CNI 后扫描 netns；
- 将得到的 `NetworkDevice` 和动态 MAC 加入启动前 GVM 配置；
- 普通 OCI hook 仍保持原执行顺序。

本地验证结果：

```text
cargo check -p shim --bin containerd-shim-kata-v2: PASS
新增的 2 个 hook/netns 单元测试: PASS
```

该提交尚未在实际服务器上完成端到端验证。

## 5. 当前主要问题

### 5.1 nerdctl 日志 URI 不兼容

`nerdctl run -d` 默认传入类似下面的日志 URI：

```text
binary:///usr/local/bin/nerdctl?_NERDCTL_INTERNAL_LOGGING=...
```

runtime-rs 当前在解析 URI 前，直接把 stdout 当成本地 FIFO 路径打开，因此报错：

```text
open stdout: No such file or directory
```

临时绕过：

```text
--log-driver none
```

正式方案需要修改 runtime-rs 的进程 IO 层，支持 containerd shim logger URI，或者至少正确区分 FIFO 路径和非 FIFO URI。

### 5.2 GVM 生命周期和 VM ID

当前 Shyper adapter 在以下位置硬编码 VM ID `1`：

- `vm boot 1`；
- `vm destroy 1`；
- `vm replace-disk 1 ...`。

第二次启动时 Hypervisor 曾分配 `vmid 2`，但 Kata 仍会操作 VM1。还观察到第二次 `shyper vm config` 已在内核创建 VM2，但命令一直不返回。

需要：

1. `vm config` 返回真实 VM ID；
2. Kata 保存 VM ID 并用于 boot/destroy/replace-disk；
3. 确认 destroy 会释放 VM 配置和 ID；
4. 修复 Shyper daemon 退出后重新启动时的命令响应问题；
5. 清理失败时不能留下 shim、daemon 和错误的 containerd RUNNING 状态。

### 5.3 多容器尚不支持

当前只能把“重启 MVM 后第一次启动的 GVM”作为可靠测试对象。尚不支持：

- 顺序启动第二个 GVM；
- 同时运行多个 GVM；
- 多 GVM 的唯一 VM ID、vsock CID、CPU、内存、MAC 和磁盘资源分配；
- daemon 的全局/每 VM 生命周期管理。

在这些问题修复前，不能认为已经具备运行 mini-swe-agent 的条件。

## 6. 开发、编译和安装流程

### 6.1 同步代码

先把 GitHub 的最新提交同步到 Gitee。在有 Gitee 凭据的机器上执行：

```bash
cd /path/to/kata-containers
git fetch origin feat-kunlun-network
git switch feat-kunlun-network
git merge --ff-only origin/feat-kunlun-network
git push gitee feat-kunlun-network
```

服务器执行：

```bash
cd /root/work/code/kata-containers
git switch feat-kunlun-network
git pull --ff-only gitee feat-kunlun-network
git rev-parse --short HEAD
```

测试 `f16b18c6d` 时，HEAD 至少应为：

```text
f16b18c6d
```

### 6.2 编译 runtime-rs

服务器执行：

```bash
cd /root/work/code/kata-containers

make -B -C src/runtime-rs \
  HYPERVISOR=shyper \
  PREFIX=/opt/kata
```

构建产物：

```text
/root/work/code/kata-containers/src/runtime-rs/target/aarch64-unknown-linux-musl/release/containerd-shim-kata-v2
```

### 6.3 安装 shim

只安装 runtime，避免覆盖正在使用的 Kata 配置文件：

```bash
make -C src/runtime-rs \
  HYPERVISOR=shyper \
  PREFIX=/opt/kata \
  install-runtime
```

实际使用的 shim：

```text
/opt/kata/runtime-rs/bin/containerd-shim-kata-v2
```

`/usr/local/bin/containerd-shim-kata-v2` 应链接到该文件。

不要使用普通的 `make install`，它可能覆盖 `/opt/kata/share/defaults/kata-containers/runtime-rs/` 下的配置。

### 6.4 验证安装结果

```bash
BUILD_SHIM=/root/work/code/kata-containers/src/runtime-rs/target/aarch64-unknown-linux-musl/release/containerd-shim-kata-v2
ACTIVE_SHIM=/opt/kata/runtime-rs/bin/containerd-shim-kata-v2

"$ACTIVE_SHIM" --version
sha256sum "$ACTIVE_SHIM" "$BUILD_SHIM"
cmp -s "$ACTIVE_SHIM" "$BUILD_SHIM"
echo "cmp result=$?"
```

预期两个 SHA-256 相同，并且：

```text
cmp result=0
```

## 7. 测试环境配置

### 7.1 持久配置

以下文件重启后仍存在。

`/etc/nerdctl/nerdctl.toml`：

```toml
namespace = "default"
snapshotter = "devmapper"
cni_path = "/opt/cni/bin"
cni_netconfpath = "/etc/cni/net.d"
dns = ["202.112.128.50", "202.112.128.51"]
```

`/etc/cni/net.d/10-shyper.conflist`：

```json
{
  "cniVersion": "1.0.0",
  "name": "shyper",
  "plugins": [
    {
      "type": "bridge",
      "bridge": "shyper0",
      "isGateway": true,
      "ipMasq": true,
      "hairpinMode": true,
      "ipam": {
        "type": "host-local",
        "ranges": [[{
          "subnet": "172.30.0.0/24",
          "rangeStart": "172.30.0.2",
          "rangeEnd": "172.30.0.254",
          "gateway": "172.30.0.1"
        }]],
        "routes": [{ "dst": "0.0.0.0/0" }]
      }
    },
    {
      "type": "portmap",
      "capabilities": { "portMappings": true }
    }
  ]
}
```

CNI 插件位于：

```text
/opt/cni/bin/bridge
/opt/cni/bin/host-local
/opt/cni/bin/loopback
/opt/cni/bin/portmap
```

### 7.2 每次重启后恢复运行时状态

以下内容重启后会丢失：devmapper 映射、`shyper.ko`、网桥状态、IP 转发和 iptables 规则。

```bash
sh /root/work/reload.sh
systemctl restart containerd

lsmod | grep -q '^shyper ' || \
insmod /root/taishan200-boot/gvm/shyper.ko

UPLINK=$(ip route show default | awk '/^default/ {print $5; exit}')
echo "UPLINK=$UPLINK"

ip link show shyper0 >/dev/null 2>&1 || \
ip link add shyper0 type bridge

ip addr flush dev eth0
ip link set eth0 up
ip link set eth0 master shyper0
ip link set shyper0 up
ip addr replace 172.30.0.1/24 dev shyper0

sysctl -w net.ipv4.ip_forward=1

iptables -t nat -C POSTROUTING \
  -s 172.30.0.0/24 -o "$UPLINK" -j MASQUERADE 2>/dev/null || \
iptables -t nat -A POSTROUTING \
  -s 172.30.0.0/24 -o "$UPLINK" -j MASQUERADE

iptables -C FORWARD \
  -i shyper0 -o "$UPLINK" -j ACCEPT 2>/dev/null || \
iptables -A FORWARD \
  -i shyper0 -o "$UPLINK" -j ACCEPT

iptables -C FORWARD \
  -i "$UPLINK" -o shyper0 \
  -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT 2>/dev/null || \
iptables -A FORWARD \
  -i "$UPLINK" -o shyper0 \
  -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
```

检查：

```bash
ctr plugins ls | grep devmapper
lsmod | grep shyper
ip -br addr show shyper0
pgrep -af 'containerd-shim-kata|shyper' || true
```

启动第一个测试 GVM 前，最后一条不应存在旧 shim 或 Shyper daemon。

## 8. 网络端到端测试

由于日志 URI 问题，必须暂时使用 `--log-driver none`：

```bash
timeout -k 5 120 nerdctl run \
  --runtime io.containerd.run.kata.v2 \
  --network shyper \
  --log-driver none \
  -d \
  --name shyper-net-test \
  docker.io/library/busybox:latest \
  sleep 3600
```

成功后立即测试，不要先停止或删除容器：

```bash
nerdctl exec shyper-net-test /bin/sh -c '
ip addr
ip route
cat /etc/resolv.conf || true
ping -c 3 -W 2 172.30.0.1
ping -c 3 -W 2 1.1.1.1
wget -T 10 -O /dev/null http://example.com && echo NETWORK_OK
'
```

预期：

- `eth0` 为 UP；
- `eth0` 获得 `172.30.0.x/24`；
- 存在 `default via 172.30.0.1`；
- 网关、外部 IP、DNS 和 HTTP 均正常。

确认新时序修复是否生效：

```bash
journalctl -u containerd --since '-5 min' --no-pager | \
grep -E 'execute nerdctl network hook|Adding device.*Network|dynamic Shyper|update interface|update routes'
```

预期出现：

```text
execute nerdctl network hook before starting Shyper VM
Adding device ... Network
Using dynamic Shyper guest MAC
update interface
update routes
```

检查生成的 Shyper 网络配置，无需安装 `jq`：

```bash
CID=$(nerdctl inspect --format '{{.Id}}' shyper-net-test)
CFG=/root/taishan200-boot/gvm/kata_gen_${CID}.json
grep -o '{[^{}]*"type":"EMU_DEVICE_T_VIRTIO_NET"[^{}]*}' "$CFG"
```

`cfg_list` 应为 CNI 分配的动态 MAC，不应继续固定为：

```text
74:56:aa:0f:47:d1
```

## 9. 故障信息采集

启动失败或超时后不要立即启动第二个容器，先采集：

```bash
nerdctl ps -a
ctr -n default tasks ls
pgrep -af 'containerd-shim-kata|shyper' || true

journalctl -u containerd --since '-10 min' --no-pager | \
tail -n 400

dmesg | \
grep -E 'VM\[|vmid|destroy|delete|Shyper daemon|HVC' | \
tail -n 200
```

nerdctl CNI hook 日志：

```bash
HOOK_LOG=$(find /var/lib/nerdctl \
  -type f -name 'oci-hook.createRuntime.log' \
  -mmin -20 2>/dev/null | head -n 1)
echo "$HOOK_LOG"
[ -n "$HOOK_LOG" ] && sed -n '1,240p' "$HOOK_LOG"
```

如果出现 task、guest 进程、shim 和 daemon 状态不一致，不要继续启动第二个 GVM。当前最安全的恢复方式是重启 MVM。

## 10. 后续开发优先级

1. 在服务器验证 `f16b18c6d` 的自动 CNI 网络；
2. 修复 runtime-rs 对 nerdctl `binary://` 日志 URI 的处理；
3. 获取 Shyper CLI、daemon 和内核模块源码；
4. 让 `vm config` 返回真实 VM ID并由 Kata 保存；
5. 移除 boot/destroy/replace-disk 中硬编码的 VM ID 1；
6. 修复 stop/remove/cleanup，确保 VM ID、daemon、shim 和磁盘全部释放；
7. 验证连续启动两个容器；
8. 实现唯一 vsock CID、MAC、CPU、内存和磁盘资源分配；
9. 验证两个并发容器及容器间网络；
10. 使用 nerdctl Compose 或兼容入口运行 mini-swe-agent 的最小任务。

在第 6 步完成以前，不要把“第一次容器能启动”视为完整的容器生命周期支持。
