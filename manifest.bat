@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul

echo ======================================================
echo           CirCube发布器
echo ======================================================

:input_manifest_ver
set /p "MANIFEST_VER=请输入清单版本号 (manifest_version): "
if "%MANIFEST_VER%"=="" goto :input_manifest_ver

:input_app_ver
set /p "APP_VER=请输入当前版本名 (version): "
if "%APP_VER%"=="" goto :input_app_ver

set "TARGET_DIR=public/updater/.minecraft"
set "OUTPUT_FILE=public/updater/launcher/manifest.json"
set "ZIP_FILE=public/CirCube.zip"

echo [System] 正在扫描: %TARGET_DIR%...

if not exist public\updater\launcher (
mkdir public\updater\launcher
)

:: 生成 manifest
pwsh -NoProfile -ExecutionPolicy Bypass -Command "$target=[System.IO.Path]::GetFullPath('%TARGET_DIR%');$files=Get-ChildItem -LiteralPath $target -Recurse -File;$manifest=[ordered]@{manifest_version='%MANIFEST_VER%';version='%APP_VER%';files=[ordered]@{}};foreach($f in $files){$relPath=$f.FullName.Replace($target+'\','').Replace('\','/');Write-Host('[处理中] '+$relPath) -ForegroundColor Cyan;try{$hash=(Get-FileHash -LiteralPath $f.FullName -Algorithm SHA1).Hash.ToLower();$manifest.files[$relPath]=@{hash=$hash;size=$f.Length}}catch{Write-Host('[失败] '+$relPath) -ForegroundColor Red}};$manifest|ConvertTo-Json -Depth 10 -Compress|Out-File -FilePath '%OUTPUT_FILE%' -Encoding utf8NoBOM"

echo ------------------------------------------------------
echo [成功] 清单已生成至: %OUTPUT_FILE%

echo [System] 正在打包 updater...

if exist "%ZIP_FILE%" del "%ZIP_FILE%"

pwsh -NoProfile -Command ^
"Compress-Archive -Path 'public/updater/*' -DestinationPath '%ZIP_FILE%' -Force"

echo [成功] 已生成: %ZIP_FILE%

echo ------------------------------------------------------
echo [System] 上传到 R2...

rclone sync ./public R2:circube/public --local-encoding None --s3-encoding None --transfers=8 --checkers=16 --progress --stats-one-line

echo [成功] R2 同步完成

echo ------------------------------------------------------
echo [System] 更新 Gitee manifest...

copy /Y public\updater\launcher\manifest.json CirCube\manifest.json >nul
cd CirCube
git add .
git commit -m "update manifest %APP_VER%"
git push

echo [完成] 所有任务完成
pause