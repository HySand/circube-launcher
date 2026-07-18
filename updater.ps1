param(
    [string]$ProjectRoot
)

$ErrorActionPreference = "Stop"

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [Console]::OutputEncoding

function Write-Section {
    param([string]$Text)
    Write-Host "------------------------------------------------------"
    Write-Host $Text
}

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,

        [Parameter(Mandatory = $true)]
        [string]$ErrorMessage
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$ErrorMessage (exit code $LASTEXITCODE)"
    }
}

function Get-RequiredCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [string]$PreferredPath
    )

    if ($PreferredPath -and (Test-Path -LiteralPath $PreferredPath -PathType Leaf)) {
        return $PreferredPath
    }

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        throw "Command not found: $Name"
    }

    return $command.Source
}

$scriptPath = $PSCommandPath
if ([string]::IsNullOrWhiteSpace($scriptPath)) {
    $scriptPath = $MyInvocation.MyCommand.Path
}

$scriptDir = if ([string]::IsNullOrWhiteSpace($scriptPath)) {
    (Get-Location).Path
} else {
    Split-Path -Parent $scriptPath
}

$knownProjectRoot = "C:\Users\zephyr\WebstormProjects\circube-launcher"

$scriptRoot = if (-not [string]::IsNullOrWhiteSpace($ProjectRoot)) {
    [System.IO.Path]::GetFullPath($ProjectRoot)
} elseif ((-not [string]::IsNullOrWhiteSpace($scriptDir)) -and (Test-Path -LiteralPath (Join-Path $scriptDir "public\updater\.minecraft") -PathType Container)) {
    $scriptDir
} elseif (Test-Path -LiteralPath (Join-Path $knownProjectRoot "public\updater\.minecraft") -PathType Container) {
    $knownProjectRoot
} elseif (-not [string]::IsNullOrWhiteSpace($scriptDir)) {
    $scriptDir
} else {
    throw "Project root cannot be resolved. Run with -ProjectRoot `"C:\Users\zephyr\WebstormProjects\circube-launcher`"."
}

if ([string]::IsNullOrWhiteSpace($scriptRoot)) {
    throw "Project root is empty. Run with -ProjectRoot `"C:\Users\zephyr\WebstormProjects\circube-launcher`"."
}
Set-Location -LiteralPath $scriptRoot

Write-Host "======================================================"
Write-Host "          CirCube Publisher"
Write-Host "======================================================"

$targetDir = Join-Path $scriptRoot "public\updater\.minecraft"
$outputFile = Join-Path $scriptRoot "public\updater\launcher\manifest.json"
$zipFile = Join-Path $scriptRoot "public\CirCube.7z"

$versionsDir = Join-Path $targetDir "versions"
if (-not (Test-Path -LiteralPath $versionsDir -PathType Container)) {
    $versionsDir = Join-Path $targetDir "version"
}

if (-not (Test-Path -LiteralPath $versionsDir -PathType Container)) {
    throw ("Version directory not found: " + (Join-Path $targetDir "versions") + " or " + (Join-Path $targetDir "version"))
}

$versionDir = Get-ChildItem -LiteralPath $versionsDir -Directory |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if ($null -eq $versionDir) {
    throw "Version directory is empty: $versionsDir"
}

$manifestVersion = $versionDir.Name
$now = Get-Date
$appVersion = "{0}{1}{2:00}{3:00}{4:00}" -f ($now.Year % 100), $now.Month, $now.Day, $now.Hour, $now.Minute

Write-Host ("System manifest_version: " + $manifestVersion)
Write-Host ("System version: " + $appVersion)
Write-Host ("System scanning: " + $targetDir + "...")

$launcherDir = Split-Path -Parent $outputFile
New-Item -ItemType Directory -Force -Path $launcherDir | Out-Null

$target = Get-Item -LiteralPath $targetDir
$trimChars = [char[]]("/", "\")
$targetPrefix = $target.FullName.TrimEnd($trimChars) + [System.IO.Path]::DirectorySeparatorChar
$files = Get-ChildItem -LiteralPath $target.FullName -Recurse -File

$manifest = [ordered]@{
    manifest_version = $manifestVersion
    version = $appVersion
    files = [ordered]@{}
}

foreach ($file in $files) {
    $relPath = $file.FullName.Substring($targetPrefix.Length).Replace("\", "/")
    Write-Host ("Processing " + $relPath) -ForegroundColor Cyan
    try {
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA1).Hash.ToLowerInvariant()
        $manifest.files[$relPath] = [ordered]@{
            hash = $hash
            size = $file.Length
        }
    } catch {
        Write-Host ("Failed " + $relPath) -ForegroundColor Red
        throw
    }
}

$json = $manifest | ConvertTo-Json -Depth 10 -Compress
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($outputFile, $json, $utf8NoBom)

Write-Section ("Success manifest generated: " + $outputFile)

$rclone = Get-RequiredCommand -Name "rclone" -PreferredPath (Join-Path $scriptDir "rclone.exe")

Write-Section "System uploading updater files to R2..."
Invoke-Native `
    -FilePath $rclone `
    -Arguments @(
        "sync",
        "./public/updater",
        "R2:circube/public/updater",
        "--local-encoding", "None",
        "--s3-encoding", "None",
        "--transfers=8",
        "--checkers=16",
        "--progress",
        "--stats-one-line",
        "--retries", "3"
    ) `
    -ErrorMessage "R2 sync failed"

Write-Host "Success R2 sync complete"

Write-Section "System uploading updater files to Bitiful..."
Invoke-Native `
    -FilePath $rclone `
    -Arguments @(
        "sync",
        "./public/updater",
        "bitiful:circube/public/updater",
        "--local-encoding", "None",
        "--s3-encoding", "None",
        "--transfers=8",
        "--checkers=16",
        "--progress",
        "--stats-one-line",
        "--retries", "3"
    ) `
    -ErrorMessage "Bitiful sync failed"

Write-Host "Success Bitiful sync complete"

Write-Section "System updating Gitee manifest..."

$giteeDir = Join-Path $scriptRoot "CirCube"
if (-not (Test-Path -LiteralPath $giteeDir -PathType Container)) {
    throw "Gitee repository directory not found: $giteeDir"
}

Copy-Item -LiteralPath $outputFile -Destination (Join-Path $giteeDir "manifest.json") -Force

Push-Location -LiteralPath $giteeDir
try {
    Invoke-Native -FilePath "git" -Arguments @("add", ".") -ErrorMessage "git add failed"
    Invoke-Native -FilePath "git" -Arguments @("commit", "-m", "update manifest $appVersion") -ErrorMessage "git commit failed"
    Invoke-Native -FilePath "git" -Arguments @("push") -ErrorMessage "git push failed"
} finally {
    Pop-Location
}

Write-Section "System packing updater..."

if (Test-Path -LiteralPath $zipFile -PathType Leaf) {
    Write-Host "Deleting existing archive: $zipFile"
    Remove-Item -LiteralPath $zipFile -Force
}

$sevenZip = Get-RequiredCommand -Name "7z" -PreferredPath "C:\Program Files\7-Zip\7z.exe"
Invoke-Native `
    -FilePath $sevenZip `
    -Arguments @("a", "-t7z", "-mx=9", "-m0=lzma2", "-md=1024m", "-mfb=273", "-myx=9", "-mqs=on", "-ms=on", "-mtc=on", $zipFile, "./public/updater/*") `
    -ErrorMessage "7-Zip failed"

Write-Host ("Success archive generated: " + $zipFile)

Write-Section "System uploading archive to R2..."
Invoke-Native `
    -FilePath $rclone `
    -Arguments @(
        "copy",
        $zipFile,
        "R2:circube/public",
        "--local-encoding", "None",
        "--s3-encoding", "None",
        "--transfers=1",
        "--checkers=4",
        "--progress",
        "--stats-one-line",
        "--retries", "3"
    ) `
    -ErrorMessage "R2 archive upload failed"

Write-Host "Success R2 archive upload complete"

Write-Host "Done"
