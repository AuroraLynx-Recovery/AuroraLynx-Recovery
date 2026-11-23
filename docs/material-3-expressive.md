# AuroraLynx Material 3 Expressive

AuroraLynx 的 Rust recovery 提供 Material 3 Expressive 和 Miuix。两种风格均可独立选择浅色或深色，使用现有 recovery 设置文件保存。MD3E 设有设备状态、常规刷机、高级刷机和设置四个主页面。界面参考 [SukiSU Nova 的浮动导航](https://github.com/SukiSU-Nova/SukiSU-Nova/blob/1439545f12bffe0263690ba862139b70ee5f20cf/manager/app/src/main/java/com/sukisu/nova/ui/component/bottombar/BottomBarMaterial.kt) 和 [分段控件](https://github.com/SukiSU-Nova/SukiSU-Nova/blob/1439545f12bffe0263690ba862139b70ee5f20cf/manager/app/src/main/java/com/sukisu/nova/ui/component/material/SegmentedList.kt)。

## 功能

- 设备状态显示数据解锁状态、槽位、电量、OTA 合并状态和当前系统。电池绘制复用 AOSP Android 17 的矢量路径和充电标记。
- 常规刷机从本机、USB 或 ADB 进入现有安装流程。本机和 USB 的文件选择器在此入口筛选 ZIP。系统更新和 recovery 更新页均可进入 WLAN。备份、恢复和设备操作沿用现有流程与确认。
- 高级刷机分开提供提取分区和刷入分区入口。后者的本机和 USB 选择器筛选 IMG，复用现有镜像解析、目标分区选择和滑动确认。在此页也可以挂载、管理离线 Root 模块、切槽和使用维护工具。
- 设置集中提供 WLAN、外观、显示与振动、语言与时区、USB 文件传输、关于和恢复凭据。WLAN 入口共享扫描、开关及连接状态。
- MD3E 的外观页可切换风格和深浅色，选择默认颜色、手动六位种子色或壁纸取色。PNG、JPEG 和 WebP 可从本机或 USB 导入。Recovery 按实际屏幕裁切范围用 Celebi 和 Score 提取种子，再按 Tonal Spot 2025 派生色板。壁纸与取色来源分别设置。卡片和底栏透明度可在 0–35% 调整，默认 15%。
- 壁纸解码限制为 24 MiB、32 MP、边长最多 8192 像素。Recovery 将归一化副本写入自己的存储目录，文件同步后更新设置引用。图片不可用时沿用保存的种子色和纯色背景。移除壁纸保留手动种子色，恢复默认重置所有外观选项。

## 重放与验收

1. 运行 `bash scripts/fetch-skyward.sh`，确认固定基线和当前补丁栈可以重放。从命令输出进入 Rust 源码目录，执行 `cargo test --locked -p skyward-ui -p skyward-preview`。
2. 执行 `cargo run --locked -p skyward-preview -- <输出目录> --style-matrix --lang zh-Hans --wallpaper <样图>`。检查四种风格与深浅色组合、四个首页、外观页、分区页，以及导航变换的定时帧。带壁纸的预览由 Rust 的实际合成路径生成。
3. 按 [上游来源与产品构建说明](UPSTREAM.md) 构建 Cerro recovery，记录 ELF、镜像哈希、体积和签名。只有设备、槽位、镜像兼容性与回滚路径确认后，才开始真机操作。
4. 真机分别验证开机解锁帧、四组合切换与重启保存、硬件键和触控、低电和充电、不同明暗的壁纸、导入失败及回退、WLAN、MTP 和分区操作的确认。核对首帧耗时、峰值内存和导航响应，完成后记录逐机结果。

## 发布仓库

项目后续的发布仓库为 [AuroraLynx-Recovery/AuroraLynx-Recovery](https://github.com/AuroraLynx-Recovery/AuroraLynx-Recovery)。2026-11-23 创建为私有空仓库，GitHub 返回唯一拥有仓库访问权限的账号为 `ycrrongos`，未添加团队。发布物应从 AuroraLynx 的可重放源码生成，并保留上游许可及版权。GitHub 凭据由本地持有，不写入代码、配置、构建结果或提交。
