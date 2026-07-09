@echo off
REM Double-click this file to build release installers for ALL desktop
REM platforms (macOS, Linux, Windows) via the "Release" GitHub Actions workflow.
REM A single machine cannot cross-build the native BLE core, so this triggers
REM CI, which builds each platform on its own native runner.
REM
REM Requirements: GitHub CLI (gh) installed and authenticated (gh auth login).

setlocal
cd /d "%~dp0.."

set "WORKFLOW=release.yml"

echo ==================================================
echo   Evolve Calc - Release all platforms
echo ==================================================
echo.

where gh >nul 2>nul
if errorlevel 1 (
  echo GitHub CLI ^(gh^) is not installed. Install from https://cli.github.com
  pause
  exit /b 1
)

gh auth status >nul 2>nul
if errorlevel 1 (
  echo GitHub CLI is not authenticated. Run: gh auth login
  pause
  exit /b 1
)

for /f "delims=" %%b in ('git rev-parse --abbrev-ref HEAD 2^>nul') do set "BRANCH=%%b"
if "%BRANCH%"=="" set "BRANCH=main"
echo Repository branch: %BRANCH%
echo.

REM Auto-increment the version from the tags already on GitHub.
for /f "delims=" %%v in ('node scripts\next-version.mjs --latest 2^>nul') do set "LATEST=%%v"
if not defined LATEST set "LATEST=none"
for /f "delims=" %%v in ('node scripts\next-version.mjs patch 2^>nul') do set "NEXT=%%v"
if not defined NEXT set "NEXT=v0.1.0"

echo Latest published tag: %LATEST%
echo.
echo Choose a version to publish:
echo   - press Enter    : auto-increment to %NEXT%
echo   - minor / major  : auto-bump that level
echo   - vX.Y.Z         : a specific version
echo   - none           : just build installers, don't publish a Release
set "INPUT="
set /p "INPUT=Version [%NEXT%]: "

set "VERSION="
if "%INPUT%"=="" (
  set "VERSION=%NEXT%"
) else if /i "%INPUT%"=="none" (
  set "VERSION="
) else if /i "%INPUT%"=="patch" (
  set "VERSION=%NEXT%"
) else if /i "%INPUT%"=="minor" (
  for /f "delims=" %%v in ('node scripts\next-version.mjs minor 2^>nul') do set "VERSION=%%v"
) else if /i "%INPUT%"=="major" (
  for /f "delims=" %%v in ('node scripts\next-version.mjs major 2^>nul') do set "VERSION=%%v"
) else (
  set "VERSION=%INPUT%"
)

echo.
echo Triggering the Release workflow on '%BRANCH%'...
if "%VERSION%"=="" (
  gh workflow run "%WORKFLOW%" --ref "%BRANCH%"
) else (
  gh workflow run "%WORKFLOW%" --ref "%BRANCH%" -f version="%VERSION%"
)

echo Opening the Actions run in your browser...
timeout /t 5 >nul
for /f "delims=" %%i in ('gh run list --workflow^=%WORKFLOW% --limit 1 --json databaseId --jq ".[0].databaseId" 2^>nul') do set "RUN_ID=%%i"
if defined RUN_ID (
  gh run view %RUN_ID% --web >nul 2>nul
) else (
  gh repo view --web >nul 2>nul
)

echo.
echo Note: workflow_dispatch only works once release.yml exists on the
echo repository's DEFAULT branch (main). If the trigger was rejected, merge
echo to main first, then run this again.
echo.
pause
endlocal
