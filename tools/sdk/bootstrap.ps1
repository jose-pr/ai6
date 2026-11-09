# Installs what tools/sdk/sdk.lock names under one directory, verified, by version. Never touches
# the shell profile, PATH or the system, and never elevates. tools/sdk/README.md has the details.
#
# Usage: bootstrap.ps1 [--groups core,app] [--prefix DIR] [--check] [--print-system]
#
# Runs on Windows PowerShell 5.1, which is all a clean Windows has, as well as on PowerShell 7.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

function Die([string] $message) {
    [Console]::Error.WriteLine("bootstrap: $message")
    exit 1
}

if ($env:OS -ne 'Windows_NT') { Die 'use bootstrap.sh on Linux and macOS' }

$here = $PSScriptRoot
$lockPath = Join-Path $here 'sdk.lock'
$groups = @('core', 'app')
$prefix = if ($env:AI6_SDK_HOME) { $env:AI6_SDK_HOME } else { Join-Path $HOME '.ai6-sdk' }
$mode = 'install'
$script:missing = $false

for ($i = 0; $i -lt $args.Count; $i++) {
    switch ($args[$i]) {
        '--groups' {
            if ($i + 1 -ge $args.Count) { Die '--groups needs a value' }
            # PowerShell itself reads an unquoted core,app as an array.
            $groups = (($args[++$i] -join ',') -split ',') | Where-Object { $_ }
        }
        '--prefix' {
            if ($i + 1 -ge $args.Count) { Die '--prefix needs a value' }
            $prefix = [string] $args[++$i]
        }
        '--check' { $mode = 'check' }
        '--print-system' { $mode = 'print-system' }
        { $_ -in '-h', '--help' } {
            Get-Content -LiteralPath $PSCommandPath -TotalCount 4 | ForEach-Object { $_ -replace '^# ?', '' }
            exit 0
        }
        default { Die "unknown argument: $($args[$i])" }
    }
}
foreach ($group in $groups) {
    if ($group -notin 'core', 'app', 'api', 'docs', 'test') {
        Die "unknown group: $group (core, app, api, docs, test)"
    }
}
if ($prefix.Contains("'")) { Die "a prefix containing a single quote cannot be written to env.ps1: $prefix" }
$prefix = [IO.Path]::GetFullPath($prefix)

# The process's own architecture can be an emulated one, so ask the machine.
$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture
switch ($cpu) {
    9 { $arch = 'x64'; $triple = 'x86_64-pc-windows-msvc' }
    12 { $arch = 'arm64'; $triple = 'aarch64-pc-windows-msvc' }
    default { Die "no SDK is listed for processor architecture $cpu" }
}
$platform = "windows-$arch"

$lock = foreach ($line in Get-Content -LiteralPath $lockPath) {
    if ($line -match '^\s*(#|$)') { continue }
    $fields = $line -split '\s+'
    [pscustomobject] @{
        Group = $fields[0]; Name = $fields[1]; Version = $fields[2]
        Platform = $fields[3]; Sha256 = $fields[4]; Url = $fields[5]
    }
}

function LockItems([string] $group, [string] $name) {
    @($lock | Where-Object { $_.Group -eq $group -and $_.Name -eq $name -and ($_.Platform -eq $platform -or $_.Platform -eq 'any') })
}

function LockItem([string] $group, [string] $name) {
    (LockItems $group $name)[0]
}

function Report([string] $what, [string] $state) {
    Write-Host ('{0,-32} {1}' -f $what, $state)
}

function Need([string] $what, [string] $state) {
    $script:missing = $true
    Report $what $state
}

# Runs a native command, stopping on a nonzero exit. Windows PowerShell turns a native command's
# redirected stderr into a terminating error under Stop, so the preference is lowered around it.
function Run([string] $exe) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & $exe @args
    $code = $LASTEXITCODE
    $ErrorActionPreference = $saved
    if ($code -ne 0) { Die "$exe $($args -join ' ') exited with $code" }
}

function Capture([string] $exe) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $out = & $exe @args 2>$null
    $ErrorActionPreference = $saved
    $out
}

function KeptPath($item) {
    $base = $item.Url.Substring($item.Url.LastIndexOf('/') + 1)
    Join-Path $prefix (Join-Path "dl\$($item.Name)-$($item.Version)" $base)
}

# Hashes a lock item's kept file, when one is there: a copy that changed on disk is refused rather
# than trusted.
function VerifyKept($item) {
    $file = KeptPath $item
    if (-not (Test-Path -LiteralPath $file)) { return }
    $actual = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $item.Sha256) {
        Remove-Item -LiteralPath $file -Force
        Die "refused and removed $file`n  sdk.lock says $($item.Sha256)`n  the file was  $actual"
    }
}

# Returns the verified file for a lock item, downloading only when no copy is kept.
function FetchVerified($item) {
    VerifyKept $item
    $file = KeptPath $item
    if (Test-Path -LiteralPath $file) { return $file }
    Report "$($item.Name) $($item.Version)" "fetching $($item.Url)"
    $tmp = Join-Path $prefix ('tmp.' + [IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $partial = Join-Path $tmp ([IO.Path]::GetFileName($file))
        Invoke-WebRequest -UseBasicParsing -Uri $item.Url -OutFile $partial
        $actual = (Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $item.Sha256) {
            Remove-Item -LiteralPath $tmp -Recurse -Force
            Die "refused $($item.Url)`n  sdk.lock says $($item.Sha256)`n  the download  $actual"
        }
        New-Item -ItemType Directory -Path (Split-Path $file) -Force | Out-Null
        Move-Item -LiteralPath $partial -Destination $file
    } finally {
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
    }
    $file
}

$rustHome = Join-Path $prefix 'rust'
$rustupHome = Join-Path $rustHome 'rustup'
$cargoHome = Join-Path $rustHome 'cargo'
$rustup = Join-Path $cargoHome 'bin\rustup.exe'

function RustupReady {
    if (-not (Test-Path -LiteralPath $rustup)) { return $false }
    $line = Capture $rustup --version | Select-Object -First 1
    "$line" -match '^rustup (\S+)' -and $Matches[1] -eq (LockItem core rustup-init).Version
}

# Whether a toolchain is here with its components and targets. Asks only the prefix, so a run with
# nothing to do stays off the network.
function ToolchainReady([string] $toolchain, [string[]] $components, [string[]] $targets) {
    if (-not (Test-Path -LiteralPath $rustup)) { return $false }
    if (-not (Capture $rustup toolchain list | Where-Object { $_ -like "$toolchain-$triple*" })) { return $false }
    if ($components) {
        $have = Capture $rustup component list --toolchain $toolchain --installed
        foreach ($c in $components) { if (-not ($have | Where-Object { $_ -like "$c*" })) { return $false } }
    }
    if ($targets) {
        $have = Capture $rustup target list --toolchain $toolchain --installed
        foreach ($t in $targets) { if ($have -notcontains $t) { return $false } }
    }
    $true
}

function InstallRustup {
    $item = LockItem core rustup-init
    VerifyKept $item
    if (RustupReady) {
        Report "rustup $($item.Version)" 'ok'
        return
    }
    $file = FetchVerified $item
    $env:RUSTUP_INIT_SKIP_PATH_CHECK = 'yes'
    Run $file -y --no-modify-path --default-toolchain none --profile minimal
    Run $rustup set auto-self-update disable
    Report "rustup $($item.Version)" 'installed'
}

function InstallToolchain([string] $toolchain, [string[]] $components, [string[]] $targets) {
    if (ToolchainReady $toolchain $components $targets) {
        Report "rust $toolchain" 'ok'
        return
    }
    $extra = @()
    foreach ($c in $components) { $extra += '-c', $c }
    foreach ($t in $targets) { $extra += '-t', $t }
    Run $rustup toolchain install $toolchain --profile minimal --no-self-update @extra
    Report "rust $toolchain" 'installed'
}

# ring compiles with clang on Windows ARM64. Returns the directory to add to PATH: none when a clang
# is already on PATH, Visual Studio's when it has the component, the locked LLVM's otherwise.
function ClangDir {
    if (Get-Command clang -ErrorAction SilentlyContinue) { return $null }
    $vs = VsInstallationPath 'Microsoft.VisualStudio.Component.VC.Llvm.Clang'
    if ($vs) {
        $dir = Join-Path $vs 'VC\Tools\Llvm\ARM64\bin'
        if (Test-Path -LiteralPath (Join-Path $dir 'clang.exe')) { return $dir }
    }
    Join-Path $prefix "llvm\$((LockItem core llvm).Version)\bin"
}

function NeedsLockedLlvm {
    $item = LockItems core llvm
    if ($item.Count -eq 0) { return $false }
    $dir = ClangDir
    $dir -and $dir.StartsWith((Join-Path $prefix 'llvm'))
}

function InstallLlvm {
    if (-not (NeedsLockedLlvm)) { return }
    $item = LockItem core llvm
    $dir = Join-Path $prefix "llvm\$($item.Version)"
    VerifyKept $item
    if (Test-Path -LiteralPath (Join-Path $dir 'bin\clang.exe')) {
        Report "llvm $($item.Version)" 'ok'
        return
    }
    $file = FetchVerified $item
    $partial = "$dir.partial"
    if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
    New-Item -ItemType Directory -Path $partial -Force | Out-Null
    # The system's own tar: another one on PATH may read C: as a remote host.
    Run (Join-Path $env:SystemRoot 'System32\tar.exe') -xf $file -C $partial --strip-components=1
    Move-Item -LiteralPath $partial -Destination $dir
    Report "llvm $($item.Version)" 'installed'
}

function GroupCore {
    $versions = @(LockItems core rust | ForEach-Object Version)
    $components = @(LockItems core rust-component | ForEach-Object Version)
    $targets = @(LockItems core rust-target | ForEach-Object Version)
    if ($mode -eq 'check') {
        $item = LockItem core rustup-init
        if (RustupReady) { Report "rustup $($item.Version)" 'ok' } else { Need "rustup $($item.Version)" "missing, would fetch $($item.Url)" }
        foreach ($version in $versions) {
            if (ToolchainReady $version $components $targets) { Report "rust $version" 'ok' }
            else { Need "rust $version" "missing, rustup would fetch it with $($components + $targets -join ' ')" }
        }
        if (NeedsLockedLlvm) {
            $item = LockItem core llvm
            if (Test-Path -LiteralPath (Join-Path $prefix "llvm\$($item.Version)\bin\clang.exe")) { Report "llvm $($item.Version)" 'ok' }
            else { Need "llvm $($item.Version)" "no clang found, would fetch $($item.Url)" }
        }
        return
    }
    InstallRustup
    foreach ($version in $versions) { InstallToolchain $version $components $targets }
    if (-not ("$(Capture $rustup default)" -like "$($versions[0])-$triple*")) {
        Run $rustup default "$($versions[0])-$triple"
    }
    InstallLlvm
}

function GroupApi {
    $nightly = (LockItem api rust).Version
    $tool = (LockItem api cargo-public-api).Version
    $stable = (LockItems core rust)[0].Version
    $bin = Join-Path $cargoHome 'bin\cargo-public-api.exe'
    $installed = if (Test-Path -LiteralPath $bin) { "$(Capture $bin --version)" -replace '^\S+\s+', '' }
    if ($mode -eq 'check') {
        if (ToolchainReady $nightly @() @()) { Report "rust $nightly" 'ok' } else { Need "rust $nightly" 'missing, rustup would fetch it' }
        if ($installed -eq $tool) { Report "cargo-public-api $tool" 'ok' } else { Need "cargo-public-api $tool" 'missing, cargo would compile it' }
        return
    }
    InstallRustup
    InstallToolchain $stable @() @()
    InstallToolchain $nightly @() @()
    if ($installed -eq $tool) { Report "cargo-public-api $tool" 'ok' }
    else { Run $rustup run $stable cargo install cargo-public-api --version $tool --locked }
}

function GitHeadIs([string] $dir, [string] $commit) {
    if (-not (Test-Path -LiteralPath (Join-Path $dir '.git'))) { return $false }
    "$(Capture git -C $dir rev-parse HEAD)" -eq $commit
}

# A directory for a commit: a full hash would put Flutter's deepest files past Windows' path limit,
# and the two scripts lay the prefix out alike.
function Short([string] $commit) {
    $commit.Substring(0, 12)
}

function FlutterRoot {
    Join-Path $prefix "flutter\$(Short (LockItem app flutter).Version)"
}

function DepsDir([string] $commit) {
    Join-Path $prefix "deps\$(Short $commit)"
}

function PubCache([string] $commit) {
    Join-Path $prefix "pub-cache\$(Short $commit)"
}

function DepsReady([string] $commit) {
    (GitHeadIs (DepsDir $commit) $commit) -and (Test-Path -LiteralPath (Join-Path (PubCache $commit) 'hosted'))
}

function InstallDeps([string] $commit) {
    $item = LockItem app deps
    $url = if ($env:AI6_SDK_DEPS_URL) { $env:AI6_SDK_DEPS_URL } else { $item.Url }
    $dir = DepsDir $commit
    if (-not (GitHeadIs $dir $commit)) {
        $partial = "$dir.partial"
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
        New-Item -ItemType Directory -Path $partial -Force | Out-Null
        Report "deps $commit" "fetching from $url"
        Run git -C $partial init -q
        Run git -C $partial config core.autocrlf false
        # By commit, so a later commit on the branch changes nothing here.
        Run git -C $partial fetch -q --depth 1 $url $commit
        Run git -C $partial checkout -q --detach FETCH_HEAD
        if ("$(Capture git -C $partial rev-parse HEAD)" -ne $commit) { Die "deps from $url is not $commit" }
        if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
        Move-Item -LiteralPath $partial -Destination $dir
    }
    $cache = PubCache $commit
    if (-not (Test-Path -LiteralPath (Join-Path $cache 'hosted'))) {
        # A copy: pub writes into the cache it uses.
        $partial = "$cache.partial"
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
        if (Test-Path -LiteralPath $cache) { Remove-Item -LiteralPath $cache -Recurse -Force }
        New-Item -ItemType Directory -Path (Split-Path $cache) -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $dir 'pub') -Destination $partial -Recurse
        # Pub takes a git package from its checkout once the checkout has .git\pub-packages, which
        # it fills in itself; git cannot carry a .git path, so deps has none.
        $git = Join-Path $partial 'git'
        if (Test-Path -LiteralPath $git) {
            foreach ($checkout in Get-ChildItem -LiteralPath $git -Directory) {
                $marker = Join-Path $checkout.FullName '.git'
                New-Item -ItemType Directory -Path $marker -Force | Out-Null
                New-Item -ItemType File -Path (Join-Path $marker 'pub-packages') -Force | Out-Null
            }
        }
        Move-Item -LiteralPath $partial -Destination $cache
    }
    Report "deps $commit" 'ok'
}

# What Flutter reports about itself, against what the lock expects of that commit.
function VerifyFlutter([string] $root) {
    $file = Join-Path $root 'bin\cache\flutter.version.json'
    if (-not (Test-Path -LiteralPath $file)) { Die "flutter wrote no $file" }
    $reported = Get-Content -LiteralPath $file -Raw | ConvertFrom-Json
    $want = (LockItem app flutter-version).Version
    if ($reported.frameworkVersion -ne $want) { Die "flutter reports version $($reported.frameworkVersion), sdk.lock says $want" }
    $engine = (Get-Content -LiteralPath (Join-Path $root 'bin\cache\engine.stamp') -Raw).Trim()
    $want = (LockItem app flutter-engine).Version
    if ($engine -ne $want) { Die "flutter's engine is $engine, sdk.lock says $want" }
    $want = (LockItem app dart).Version
    if (-not $reported.dartSdkVersion.Contains($want)) { Die "flutter's Dart is $($reported.dartSdkVersion), sdk.lock says $want" }
}

function InstallFlutter {
    $item = LockItem app flutter
    $url = if ($env:AI6_SDK_FLUTTER_URL) { $env:AI6_SDK_FLUTTER_URL } else { $item.Url }
    $root = FlutterRoot
    $version = (LockItem app flutter-version).Version
    if (-not (GitHeadIs $root $item.Version)) {
        $partial = "$root.partial"
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
        New-Item -ItemType Directory -Path (Split-Path $root) -Force | Out-Null
        Report "flutter $($item.Version)" "cloning $url"
        # Flutter takes its version from the commits and tags, and needs a real checkout: a
        # commit-only clone gives it both and fetches just the trees and files of this revision.
        Run git -c core.autocrlf=false -c core.longpaths=true clone -q -c core.autocrlf=false -c core.longpaths=true --filter=tree:0 --no-checkout $url $partial
        Run git -C $partial checkout -q --detach $item.Version
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
        Move-Item -LiteralPath $partial -Destination $root
    }
    if ($env:AI6_SDK_FLUTTER_URL) { $env:FLUTTER_GIT_URL = $env:AI6_SDK_FLUTTER_URL }
    $env:FLUTTER_ROOT = $root
    $env:PUB_CACHE = PubCache (LockItem app deps).Version
    $env:FLUTTER_WINDOWING = 'true'
    $stamp = Join-Path $root 'bin\cache\.ai6-sdk-precache'
    if (-not (Test-Path -LiteralPath $stamp)) {
        Report "flutter $version" 'fetching its Dart and engine for windows and web'
        $flutter = Join-Path $root 'bin\flutter.bat'
        Run $flutter --version | Out-Null
        Run $flutter precache --windows --web
        VerifyFlutter $root
        New-Item -ItemType File -Path $stamp -Force | Out-Null
    } else {
        VerifyFlutter $root
    }
    Report "flutter $version" 'ok'
}

function GroupApp {
    $commit = (LockItem app deps).Version
    if ($mode -eq 'check') {
        if (DepsReady $commit) { Report "deps $commit" 'ok' }
        else { Need "deps $commit" "missing, would fetch from $(if ($env:AI6_SDK_DEPS_URL) { $env:AI6_SDK_DEPS_URL } else { (LockItem app deps).Url })" }
        $root = FlutterRoot
        $item = LockItem app flutter
        $version = (LockItem app flutter-version).Version
        if ((GitHeadIs $root $item.Version) -and (Test-Path -LiteralPath (Join-Path $root 'bin\cache\.ai6-sdk-precache'))) { Report "flutter $version" 'ok' }
        else { Need "flutter $version" "missing, would clone $(if ($env:AI6_SDK_FLUTTER_URL) { $env:AI6_SDK_FLUTTER_URL } else { $item.Url }) at $($item.Version), then its engine for windows and web" }
        return
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { Die 'git is needed for the app group; see --print-system' }
    InstallDeps $commit
    InstallFlutter
}

function GroupDocs {
    $version = (LockItem docs mkdocs).Version
    $venv = Join-Path $prefix "docs\mkdocs-$version"
    $requirements = Join-Path $here 'docs-requirements.txt'
    $kept = Join-Path $venv 'requirements.txt'
    if ((Test-Path -LiteralPath (Join-Path $venv 'Scripts\mkdocs.exe')) -and (Test-Path -LiteralPath $kept) -and
        (Get-FileHash -LiteralPath $requirements).Hash -eq (Get-FileHash -LiteralPath $kept).Hash) {
        Report "docs mkdocs $version" 'ok'
        return
    }
    if ($mode -eq 'check') {
        Need "docs mkdocs $version" 'missing, pip would fetch what docs-requirements.txt lists, hash-checked'
        return
    }
    $python = FindPython
    if (-not $python) { Die 'Python 3 is needed for the docs group; see --print-system' }
    if (Test-Path -LiteralPath $venv) { Remove-Item -LiteralPath $venv -Recurse -Force }
    New-Item -ItemType Directory -Path (Split-Path $venv) -Force | Out-Null
    Run $python -m venv $venv
    Run (Join-Path $venv 'Scripts\python.exe') -m pip install --quiet --disable-pip-version-check --require-hashes -r $requirements
    Copy-Item -LiteralPath $requirements -Destination $kept
    Report "docs mkdocs $version" 'installed'
}

# A python found by name can be the Store's installer stub, so it has to answer first.
function FindPython {
    foreach ($name in 'python3', 'python') {
        $command = Get-Command $name -ErrorAction SilentlyContinue
        if ($command -and "$(Capture $command.Source -c 'import sys; print(sys.version_info[0])')" -eq '3') {
            return $command.Source
        }
    }
    $null
}

function GroupTest {
    $item = LockItem test node
    $dir = Join-Path $prefix "node\$($item.Version)"
    $postgres = LockItem test postgres
    Report 'postgres image' "$($postgres.Url -replace '^docker://', ''):$($postgres.Version)@sha256:$($postgres.Sha256), left to your container engine"
    if (Test-Path -LiteralPath (Join-Path $dir 'node.exe')) {
        VerifyKept $item
        Report "node $($item.Version)" 'ok'
        return
    }
    if ($mode -eq 'check') {
        Need "node $($item.Version)" "missing, would fetch $($item.Url)"
        return
    }
    $file = FetchVerified $item
    $partial = "$dir.partial"
    if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
    New-Item -ItemType Directory -Path $partial -Force | Out-Null
    Expand-Archive -LiteralPath $file -DestinationPath $partial
    $inner = Get-ChildItem -LiteralPath $partial -Directory | Select-Object -First 1
    Move-Item -LiteralPath $inner.FullName -Destination $dir
    Remove-Item -LiteralPath $partial -Recurse -Force
    Report "node $($item.Version)" 'installed'
}

function VsWhere {
    $path = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $path) { $path }
}

function VsInstallationPath([string] $component) {
    $vswhere = VsWhere
    if (-not $vswhere) { return $null }
    Capture $vswhere -products * -prerelease -requires $component -property installationPath | Select-Object -First 1
}

function WindowsSdkFound {
    foreach ($key in 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots') {
        $roots = Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue
        if ($roots -and $roots.KitsRoot10 -and (Test-Path -LiteralPath (Join-Path $roots.KitsRoot10 'Include'))) { return $true }
    }
    $false
}

function SystemItemFound([string] $name) {
    switch -Wildcard ($name) {
        'Microsoft.VisualStudio.*' { return [bool] (VsInstallationPath $name) }
        'windows-sdk' { return (WindowsSdkFound) }
        'docker-or-podman' { return [bool] ((Get-Command docker -ErrorAction SilentlyContinue) -or (Get-Command podman -ErrorAction SilentlyContinue)) }
        'python3' { return [bool] (FindPython) }
        default { return [bool] (Get-Command $name -ErrorAction SilentlyContinue) }
    }
}

function PrintSystem {
    $tools = if ($arch -eq 'arm64') { 'Microsoft.VisualStudio.Component.VC.Tools.ARM64' } else { 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' }
    $clang = if ($arch -eq 'arm64') { ' --add Microsoft.VisualStudio.Component.VC.Llvm.Clang' } else { '' }
    Write-Host '# Visual Studio Build Tools, from https://aka.ms/vs/17/release/vs_BuildTools.exe; --includeRecommended brings the Windows SDK and CMake'
    Write-Host "vs_BuildTools.exe --passive --wait --includeRecommended --add Microsoft.VisualStudio.Workload.VCTools --add $tools$clang"
    Write-Host '# Git'
    Write-Host 'winget install --id Git.Git --exact'
    Write-Host '# Python, for the docs group'
    Write-Host 'winget install --id Python.Python.3.14 --exact'
    Write-Host '# Docker or Podman, for the test group, as its own installer says'
}

function CheckSystem {
    $absent = $false
    foreach ($item in ($lock | Where-Object { $_.Group -eq 'system' -and ($_.Platform -eq $platform -or $_.Platform -eq 'any') })) {
        if ($groups -notcontains $item.Version) { continue }
        if (SystemItemFound $item.Name) { Report "system $($item.Name)" 'found' }
        else {
            $absent = $true
            Report "system $($item.Name)" 'MISSING; see --print-system'
        }
    }
    if ($absent) {
        if ($mode -eq 'check') { $script:missing = $true }
        Write-Host 'bootstrap: some of the system group is missing. Nothing here installs it:'
        PrintSystem
    }
}

function WriteEnv {
    $vars = [ordered] @{ AI6_SDK_HOME = $prefix; FLUTTER_WINDOWING = 'true' }
    $paths = @()
    if (Test-Path -LiteralPath $rustup) {
        $vars.RUSTUP_HOME = $rustupHome
        $vars.CARGO_HOME = $cargoHome
        $paths += Join-Path $cargoHome 'bin'
    }
    $root = FlutterRoot
    if (Test-Path -LiteralPath (Join-Path $root '.git')) {
        $vars.FLUTTER_ROOT = $root
        $paths += Join-Path $root 'bin'
        $paths += Join-Path $root 'bin\cache\dart-sdk\bin'
        if ($env:AI6_SDK_FLUTTER_URL) { $vars.FLUTTER_GIT_URL = $env:AI6_SDK_FLUTTER_URL }
    }
    $commit = (LockItem app deps).Version
    if (Test-Path -LiteralPath (PubCache $commit)) {
        $vars.PUB_CACHE = PubCache $commit
        $vars.AI6_SDK_DEPS = DepsDir $commit
    }
    if ((LockItems core llvm).Count -gt 0) {
        $dir = ClangDir
        if ($dir -and (Test-Path -LiteralPath (Join-Path $dir 'clang.exe'))) { $paths += $dir }
    }
    $node = Join-Path $prefix "node\$((LockItem test node).Version)"
    if (Test-Path -LiteralPath (Join-Path $node 'node.exe')) { $paths += $node }
    $docs = Join-Path $prefix "docs\mkdocs-$((LockItem docs mkdocs).Version)\Scripts"
    if (Test-Path -LiteralPath (Join-Path $docs 'mkdocs.exe')) { $paths += $docs }

    $ps = @("# Written by tools/sdk/bootstrap.ps1. Source it: . '$prefix\env.ps1'")
    $sh = @("# Written by tools/sdk/bootstrap.ps1, for Git Bash. Source it: . '$(ShellPath "$prefix\env.sh")'")
    foreach ($name in $vars.Keys) {
        $ps += "`$env:$name = '$($vars[$name])'"
        $sh += "export $name='$($vars[$name])'"
    }
    $ps += "`$env:PATH = '$($paths -join ';');' + `$env:PATH"
    $sh += "export PATH='$((($paths | ForEach-Object { ShellPath $_ }) -join ':')):'`"`$PATH`""
    # LF and no byte order mark, as every text file here, and a shell cannot read one.
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText((Join-Path $prefix 'env.ps1'), (($ps -join "`n") + "`n"), $utf8)
    [IO.File]::WriteAllText((Join-Path $prefix 'env.sh'), (($sh -join "`n") + "`n"), $utf8)
}

# C:\x as Git Bash's /c/x.
function ShellPath([string] $path) {
    $slashed = $path -replace '\\', '/'
    if ($slashed -match '^([A-Za-z]):(.*)$') { return "/$($Matches[1].ToLowerInvariant())$($Matches[2])" }
    $slashed
}

if ($mode -eq 'print-system') {
    PrintSystem
    exit 0
}

# Only for this run; the same names may belong to the session that called this script.
$changed = 'RUSTUP_HOME', 'CARGO_HOME', 'RUSTUP_INIT_SKIP_PATH_CHECK', 'FLUTTER_ROOT', 'PUB_CACHE', 'FLUTTER_WINDOWING', 'FLUTTER_GIT_URL'
$saved = @{}
foreach ($name in $changed) { $saved[$name] = [Environment]::GetEnvironmentVariable($name) }
$env:RUSTUP_HOME = $rustupHome
$env:CARGO_HOME = $cargoHome
try {
    if ($mode -ne 'check') { New-Item -ItemType Directory -Path (Join-Path $prefix 'dl') -Force | Out-Null }
    Report 'prefix' $prefix
    Report 'platform' $platform
    if ($groups -contains 'core') { GroupCore }
    if ($groups -contains 'app') { GroupApp }
    if ($groups -contains 'api') { GroupApi }
    if ($groups -contains 'docs') { GroupDocs }
    if ($groups -contains 'test') { GroupTest }
    CheckSystem
    if ($mode -ne 'check') {
        WriteEnv
        Report 'env' "$prefix\env.ps1, $prefix\env.sh"
    }
} finally {
    foreach ($name in $changed) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
}
if ($mode -eq 'check' -and $script:missing) { exit 1 }
