# v1.1.0 本地发行核验

Windows x64，PowerShell 7，CPython 3.12.12，Electron 44.2.0，Node.js 22.22.0。

- 干净发行环境全量 Python 测试：560 通过、3 跳过；Electron 测试：43 通过；
  Ink 测试：79 通过，包含真实 Python bridge、审批、暂停/继续及取消。
- Desktop 和 TUI 生产构建通过，安装包内置 Node.js 与 TUI 生产依赖。
- wheel 与打包 Python 均不含旧 `minicode.ui` 或 Textual 依赖。
- 空白用户目录、无系统 Python/Git/Node 的 PATH 下，包内 CLI、启动器、Ink 模块、
  FakeProvider 执行与 bridge 正常退出通过；仓库外 B0/B2 分页评测为 2/2。
- Windows ConPTY 中实际启动包内 CLI/TUI，中文输入、审批写入、回复和正常退出通过。
- 真实 Electron 页面与 Python IPC 初始化、Git 统计及暂存通过。
- Gitleaks 历史和源码快照无发现；源码及最终打包目录 13827 个文件通过本机凭据审计。
- GitHub Windows runner 的临时目录同时存在长路径和 8.3 短路径，桥接集成测试改为
  比较实际路径，避免把同一目录误判为不同工作区。该修改只影响测试。
- 安装包 SHA256 随 Release 提供，未做商业代码签名。远端 CI 结果由 GitHub Actions
  单独记录；本页列出的测试数量来自本机完整发行构建。

## v1.0.0 本地发行核验（2026-09-30）

发行目标：Windows 10/11 x64。实际验证环境为 Windows x64、PowerShell 7、
CPython 3.12.12、Electron 44.2.0。

- TypeScript/Vite 生产构建成功。
- 使用安装包内置 Python 和锁定依赖执行全量测试：565 通过、3 跳过。
- Electron 主进程及打包路径测试：28 通过。
- 从最终安装包静默安装到包含中文与空格的路径，安装成功；开始菜单的
  Desktop/CLI 快捷方式以及卸载程序已生成。
- 在空白 USERPROFILE/APPDATA 中运行真实 Electron。应用进程的 PATH 仅保留
  Windows 系统目录，Python 桥接初始化、无密钥配置读取、工作区文件的 Git
  行数统计与暂存成功；真实渲染页面截图已检查。
- 安装后的 CLI launcher、内置 Git、Python 模块入口正常；仓库外的分页修复
  B0/B2 无密钥评测为 2/2 通过。
- Gitleaks 8.30.1 扫描全部历史与待发布源码快照，未发现凭据。
- 精确匹配本机配置和环境中的已有凭据：历史 804 个 blob、源码 366 个文件、
  最终打包目录 8156 个文件均通过。个人配置、私钥和会话数据检查通过；
  公开 CA 证书正常保留。
- 安装包 SHA256 单独随 Release 提供。安装包未做代码签名。

真实模型调用必须由下载者在设置中添加自己的服务凭据。本次安装验证使用
离线模型和已有确定性 Provider 测试，不将其描述为新的付费 API 效果评测。
GitHub Actions 的结果由远端运行另行记录，本页只记录本机验证。

安装产物只从干净 Python、受校验的 MinGit、锁定依赖和源码白名单生成；
本机虚拟环境、缓存、个人 API key 和服务配置均不随源码或安装包发布。
