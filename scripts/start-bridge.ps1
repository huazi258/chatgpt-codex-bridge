param([string]$Mode)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$checkOnly = $Mode -eq 'check'

function Fail([string]$message) {
  Write-Host "[错误] $message" -ForegroundColor Red
  if (-not $checkOnly) { Read-Host '按 Enter 关闭窗口' | Out-Null }
  exit 1
}

function Require-Command([string]$name, [string]$message) {
  if (-not (Get-Command $name -ErrorAction SilentlyContinue)) { Fail $message }
}

Write-Host ''
Write-Host 'ChatGPT x Codex Bridge Windows 启动检查' -ForegroundColor Cyan
Write-Host "目录：$root"
Write-Host ''

Require-Command node '未检测到 Node.js。请安装 Node.js LTS 后重试。'
Require-Command npm '未检测到 npm。请修复 Node.js 安装后重试。'
Require-Command cargo '未检测到 Rust/Cargo。请安装 Rust 工具链后重试。'
Write-Host "Node：$(node --version)"
Write-Host "npm：$(npm --version)"
Write-Host "Rust：$(cargo --version)"

if (Get-Command codex -ErrorAction SilentlyContinue) {
  Write-Host "Codex：$(codex --version)"
} elseif (Get-Command codex.cmd -ErrorAction SilentlyContinue) {
  Write-Host "Codex：$(codex.cmd --version)"
} else {
  Write-Host '[警告] 未检测到 Codex。请先安装并登录 Codex CLI。' -ForegroundColor Yellow
  Write-Host '        仍会启动应用，方便查看已有会话和状态。' -ForegroundColor Yellow
}

if (-not (Test-Path (Join-Path $root 'node_modules'))) {
  if ($checkOnly) { Fail '未找到 node_modules。请运行 Start Bridge.cmd 以执行 npm ci。' }
  Write-Host '未找到 node_modules，正在执行 npm ci...' -ForegroundColor Yellow
  & npm ci
  if ($LASTEXITCODE -ne 0) { Fail 'npm ci 安装依赖失败。请检查网络、npm 配置和 package-lock.json。' }
} else {
  Write-Host 'node_modules：已就绪'
}

if ($checkOnly) {
  Write-Host ''
  Write-Host '检查通过。' -ForegroundColor Green
  exit 0
}

Write-Host ''
Write-Host '正在启动桌面开发环境。关闭此窗口会停止 Bridge。' -ForegroundColor Cyan
& npm run tauri dev
if ($LASTEXITCODE -ne 0) { Fail "Bridge 已退出，错误码：$LASTEXITCODE" }
