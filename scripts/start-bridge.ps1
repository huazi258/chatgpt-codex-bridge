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

function Test-ContainsPath([string]$value, [string]$path) {
  return -not [string]::IsNullOrWhiteSpace($value) -and $value.IndexOf($path, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
}

function Get-PortProcessLookup([int]$port) {
  $pids = @()
  $netTcp = Get-Command Get-NetTCPConnection -ErrorAction SilentlyContinue
  if ($netTcp) {
    try {
      $pids = @(Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction Stop | Select-Object -ExpandProperty OwningProcess -Unique)
      return [pscustomobject]@{ Readable = $true; Pids = $pids; Error = $null }
    } catch {
      Write-Host "[提示] 无法通过 Get-NetTCPConnection 检查端口，正在尝试 netstat。" -ForegroundColor Yellow
    }
  } else {
    Write-Host "[提示] 当前系统没有 Get-NetTCPConnection，正在使用 netstat 检查端口。" -ForegroundColor Yellow
  }

  try {
    $escapedPort = [regex]::Escape([string]$port)
    $matches = @((& netstat -ano -p tcp 2>$null) | ForEach-Object {
      if ($_ -match "^\s*TCP\s+\S+:$escapedPort\s+\S+\s+LISTENING\s+(\d+)\s*$") { [int]$Matches[1] }
    } | Select-Object -Unique)
    if ($LASTEXITCODE -ne 0) { throw 'netstat failed' }
    return [pscustomobject]@{ Readable = $true; Pids = $matches; Error = $null }
  } catch {
    return [pscustomobject]@{ Readable = $false; Pids = @(); Error = $_.Exception.Message }
  }
}

function Get-ProcessDetails([int]$processId) {
  $name = '未知'
  $commandLine = $null
  try {
    $process = Get-CimInstance Win32_Process -Filter "ProcessId = $processId" -ErrorAction Stop
    if ($process) { $name = $process.Name; $commandLine = $process.CommandLine }
  } catch {
    try { $name = (Get-Process -Id $processId -ErrorAction Stop).ProcessName } catch { }
  }
  return [pscustomobject]@{ Pid = $processId; Name = $name; CommandLine = $commandLine }
}

function Get-VitePortOwners([int]$port) {
  $lookup = Get-PortProcessLookup $port
  if (-not $lookup.Readable) { return [pscustomobject]@{ Readable = $false; Owners = @(); Error = $lookup.Error } }
  return [pscustomobject]@{ Readable = $true; Owners = @($lookup.Pids | ForEach-Object { Get-ProcessDetails $_ }); Error = $null }
}

function Show-VitePortOwners([object[]]$owners) {
  Write-Host 'Vite 端口 1420：已被占用' -ForegroundColor Red
  foreach ($owner in $owners) {
    Write-Host "  PID：$($owner.Pid)"
    Write-Host "  进程：$($owner.Name)"
    if ($owner.CommandLine) {
      $summary = ($owner.CommandLine -replace '\s+', ' ').Trim()
      if ($summary.Length -gt 220) { $summary = "$($summary.Substring(0, 217))..." }
      Write-Host "  命令：$summary"
    }
  }
}

function Test-CurrentBridgeVite([object]$owner) {
  if ($owner.Name -notmatch '^(?i:node)(?:\.exe)?$') { return $false }
  return (Test-ContainsPath $owner.CommandLine $root) -and $owner.CommandLine -match '(?i)vite'
}

function Get-RunningBridgeDesktopProcesses {
  try {
    $processes = @(Get-CimInstance Win32_Process -ErrorAction Stop | Where-Object {
      $_.Name -match '^(?i:chatgpt-codex-middleware)(?:\.exe)?$' -and ((Test-ContainsPath $_.ExecutablePath $root) -or (Test-ContainsPath $_.CommandLine $root))
    })
    return [pscustomobject]@{ Reliable = $true; Processes = $processes }
  } catch {
    return [pscustomobject]@{ Reliable = $false; Processes = @() }
  }
}

function Ensure-VitePort([bool]$allowCleanup) {
  $lookup = Get-VitePortOwners 1420
  if (-not $lookup.Readable) {
    Write-Host "[错误] 无法检查 Vite 端口 1420：$($lookup.Error)" -ForegroundColor Red
    return 'unreadable'
  }
  if ($lookup.Owners.Count -eq 0) {
    Write-Host 'Vite 端口 1420：可用' -ForegroundColor Green
    return 'available'
  }

  Show-VitePortOwners $lookup.Owners
  if (-not $allowCleanup) { return 'occupied' }
  $residualVite = @($lookup.Owners | Where-Object { Test-CurrentBridgeVite $_ })
  if ($residualVite.Count -ne $lookup.Owners.Count) {
    Write-Host '端口 1420 已被其他程序占用，不会自动结束该进程。' -ForegroundColor Red
    return 'occupied'
  }

  $bridgeProcesses = Get-RunningBridgeDesktopProcesses
  if (-not $bridgeProcesses.Reliable) {
    Write-Host '无法可靠判断 Bridge/Tauri 应用是否仍在运行，不会自动结束 Vite。' -ForegroundColor Red
    return 'occupied'
  }
  if ($bridgeProcesses.Processes.Count -gt 0) {
    Write-Host 'Bridge 已经在运行。' -ForegroundColor Yellow
    return 'already-running'
  }

  Write-Host '检测到上一次残留的 Bridge Vite 开发进程，正在清理…' -ForegroundColor Yellow
  foreach ($owner in $residualVite) {
    try { Stop-Process -Id $owner.Pid -ErrorAction Stop } catch {
      Write-Host "无法结束 PID $($owner.Pid)：$($_.Exception.Message)" -ForegroundColor Red
      return 'occupied'
    }
  }
  Start-Sleep -Milliseconds 300
  $afterCleanup = Get-VitePortOwners 1420
  if ($afterCleanup.Readable -and $afterCleanup.Owners.Count -eq 0) {
    Write-Host 'Vite 端口 1420：可用' -ForegroundColor Green
    return 'available'
  }
  if ($afterCleanup.Readable) { Show-VitePortOwners $afterCleanup.Owners }
  return 'occupied'
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

$vitePortStatus = Ensure-VitePort (-not $checkOnly)
if ($vitePortStatus -eq 'already-running') { exit 0 }
if ($vitePortStatus -eq 'occupied') { Fail '端口 1420 已被占用，Bridge 开发环境无法启动。' }
if ($vitePortStatus -eq 'unreadable') { Fail '无法安全确认端口 1420 是否可用，Bridge 开发环境无法启动。' }

if ($checkOnly) {
  Write-Host ''
  Write-Host '检查通过。' -ForegroundColor Green
  exit 0
}

Write-Host ''
Write-Host '正在启动桌面开发环境。关闭此窗口会停止 Bridge。' -ForegroundColor Cyan
& npm run tauri dev
if ($LASTEXITCODE -ne 0) { Fail "Bridge 已退出，错误码：$LASTEXITCODE" }
