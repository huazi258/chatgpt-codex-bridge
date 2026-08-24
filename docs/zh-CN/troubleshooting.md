# 故障排查

Bridge 的原则是：

> 无法确认时，明确报错并暂停，而不是猜测。

因此很多看起来“为什么不自动恢复”的行为其实是刻意设计。

## 1. Bridge 启动不了

先检查：

```powershell
node --version
npm --version
cargo --version
```

重新安装 npm 依赖：

```powershell
npm install --registry=https://registry.npmjs.org
```

然后：

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
npm run tauri -- dev
```

Windows 还需要 WebView2。

## 2. 找不到 Codex

执行：

```powershell
codex --version
```

如果 PowerShell 能找到 Codex，但 Bridge 找不到：

> 很可能是桌面进程启动时拿到的 `PATH` 不一样。

尽量从已经可以正常运行 `codex` 的 PowerShell 启动 Bridge。

必要时可以根据当前开发环境设置：

```text
CODEX_APP_SERVER_COMMAND
```

为 Codex App Server 的完整可执行路径。

## 3. ChatGPT 一直显示未连接

确认：

- Bridge 已启动；
- Chrome 扩展已经加载；
- 当前页面是 `https://chatgpt.com`；
- ChatGPT 已登录；
- 使用的是本次 Bridge 进程生成的新配对密钥；
- 加载或更新扩展以后刷新过 ChatGPT 页面。

然后重新 Pair。

## 4. Bridge 重启后突然无法配对

这是正常行为。

Bridge 重启会生成新的：

```text
一次性配对密钥
```

旧密钥失效。

重新复制并 Pair 即可。

## 5. 更新扩展后感觉还是旧版本

进入：

```text
chrome://extensions
```

点击扩展的：

```text
重新加载
```

然后回到 ChatGPT 标签页：

```text
Ctrl + R
```

当前仓库中的扩展 manifest 版本是：

```text
1.3.3
```

## 6. ChatGPT 正常回复了，但 Codex 没执行

先检查这条消息是不是用：

```text
发送自动化请求
```

发送的。

如果是：

```text
手动聊天
```

那么 Bridge 永远不会解析控制块。

然后检查 ChatGPT 回复末尾是不是一个合法控制块。

例如：

```text
@@@CODEX_PROMPT@@@
...
@@@END_CODEX_PROMPT@@@
```

控制块必须位于整个回复最后。

## 7. 自动化回复被判定无效

有效控制块只有：

```text
CODEX_PROMPT
MODULE_DONE
BLOCKED
```

不要使用旧协议里的：

```text
CODEX_INPUT
```

Bridge 会根据当前模块配置的 retry template 自动重试一次。

连续两次无效后自动化会停止。

## 8. 消息变成 UNKNOWN

不要把 `UNKNOWN` 当作普通失败。

它的意思是：

> Bridge 无法确认这条消息到底有没有到 ChatGPT。

这种情况最容易出现在：

- 应用突然退出；
- 浏览器断开；
- 页面刷新；
- 消息发送过程中重启 Bridge。

此时不要盲目重发。

使用：

```text
明确重发这条消息
```

或：

```text
不重发并继续
```

做明确决定。

## 9. 重启后后面的消息都不发送

查看是否出现：

```text
待人工处理的不确定送达消息
```

只要还有 `UNKNOWN`：

> 后面的消息会继续保持安全阻塞。

把所有 UNKNOWN 都处理完。

## 10. 继续现有 Codex 对话时找不到 thread

检查：

1. Codex 工作目录是不是正确；
2. 修改路径以后有没有重新点击 **刷新对话**；
3. 目标 thread 是否正在外部 active；
4. thread 是否处于错误或不可安全接管状态。

Bridge 不会强行占用正在别处工作的 Codex thread。

## 11. 模块进入 RECOVERY_REQUIRED

表示 Bridge 对 Codex thread 当前状态没有足够确定性。

例如：

- App Server 意外退出；
- resume 操作结果不确定；
- thread 所有权无法安全判断。

使用模块内显示的恢复面板。

不要在外部连续重复 resume/start 操作。

## 12. MODULE_DONE 出现了，但是模块还没完成

这是正常的。

`MODULE_DONE` 只是：

> ChatGPT 请求用户进行验收。

模块会进入：

```text
WAITING_FOR_ACCEPTANCE
```

你仍然需要：

- 验收；
- 提反馈继续；
- 或终止。

## 13. 点击终止后 Codex 还在运行

如果当前 turn 已经开始：

Bridge 不会强行中断。

它会让这一轮运行完，但不会把这个结果继续送回 ChatGPT 自动执行下一轮。

## 14. Chrome 扩展等待 ChatGPT 超时

可能原因：

- ChatGPT 还没生成完；
- 页面被刷新；
- 扩展更新后页面没刷新；
- ChatGPT DOM 结构变化；
- content script 不是当前版本。

可以尝试：

1. Reload 扩展；
2. 刷新 ChatGPT 页面；
3. 重新 Pair；
4. 再次操作。

## 15. ChatGPT 页面更新后扩展失效

当前 adapter 依赖一些 ChatGPT 页面 selector，例如：

```text
#prompt-textarea
button[data-testid="send-button"]
button[data-testid="stop-button"]
[data-message-author-role="assistant"]
```

如果 ChatGPT 改了页面 DOM：

> Bridge 扩展可能会主动报错，而不会随便猜新的元素。

这时需要检查：

```text
spikes/chatgpt-extension/README.md
```

以及 extension adapter 代码。

## 16. Codex 操作了错误的项目

检查模块中的：

```text
Codex 工作目录
```

Bridge 不负责：

- 自动切分支；
- 自动换 repository；
- 自动寻找项目；
- 自动判断哪个目录才是正确目录。

路径填错，就应该停止模块并重新确认。

## 17. 不知道某个问题应该归 ChatGPT、Codex 还是 Bridge

可以用这条规则：

### ChatGPT

负责：

- 分析；
- 规划；
- review；
- 写 Codex Prompt。

### Codex

负责：

- 读代码；
- 改代码；
- 测试；
- 执行工程任务。

### Bridge

负责：

- 传消息；
- 消息顺序；
- thread；
- 模块；
- budget；
- delivery state；
- recovery。

### 用户

负责：

- 项目准备；
- 人工决策；
- 最终验收。
