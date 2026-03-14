@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul

echo ======================================================
echo           Minecraft 清单生成器
echo ======================================================

:input_manifest_ver
set /p "MANIFEST_VER=请输入清单版本号 (manifest_version): "
if "%MANIFEST_VER%"=="" goto :input_manifest_ver

:input_app_ver
set /p "APP_VER=请输入当前版本名 (version): "
if "%APP_VER%"=="" goto :input_app_ver

set "TARGET_DIR=.minecraft"
set "OUTPUT_FILE=manifest.json"

echo [System] 正在扫描: %TARGET_DIR%...

pwsh -NoProfile -ExecutionPolicy Bypass -Command ^
    "$target = [System.IO.Path]::GetFullPath('%TARGET_DIR%');" ^
    "$files = Get-ChildItem -LiteralPath $target -Recurse -File;" ^
    "$manifest = [ordered]@{ " ^
    "    manifest_version = '%MANIFEST_VER%';" ^
    "    version          = '%APP_VER%';" ^
    "    files            = [ordered]@{}" ^
    "};" ^
    "foreach ($f in $files) {" ^
    "    $relPath = $f.FullName.Replace($target + '\', '').Replace('\', '/');" ^
    "    Write-Host ('[处理中] ' + $relPath) -ForegroundColor Cyan;" ^
    "    try {" ^
    "        $hash = (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA1).Hash.ToLower();" ^
    "        $manifest.files[$relPath] = @{ hash = $hash; size = $f.Length };" ^
    "    } catch {" ^
    "        Write-Host ('[失败] 无法读取文件: ' + $relPath) -ForegroundColor Red;" ^
    "    }" ^
    "};" ^
    "$manifest | ConvertTo-Json -Depth 10 -Compress | Out-File -FilePath '%OUTPUT_FILE%' -Encoding utf8NoBOM"

echo ------------------------------------------------------
echo [成功] 清单已生成至: %OUTPUT_FILE%
pause