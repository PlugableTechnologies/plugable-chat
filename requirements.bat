@echo off
REM Requirements installer for Windows
REM This script calls the PowerShell script to install dependencies via winget
REM
REM Usage: Double-click this file, or run from Command Prompt: requirements.bat
REM        Add --check or --diagnose to run diagnostics without installing
REM
REM Note: The PowerShell implementation is in scripts\windows-requirements.ps1
REM       Always run this .bat file (not the .ps1 directly) to ensure proper permissions.
REM

REM Check if we're in the project root (should have package.json)
if not exist "%~dp0package.json" (
    echo.
    echo ERROR: This script must be run from the project root directory.
    echo Please navigate to the plugable-chat folder and run: requirements.bat
    echo.
    pause
    exit /b 1
)

REM Check for --check or --diagnose flags
set "PS_ARGS="
if "%~1"=="--check" set "PS_ARGS=-Check"
if "%~1"=="--diagnose" set "PS_ARGS=-Diagnose"
if "%~1"=="-check" set "PS_ARGS=-Check"
if "%~1"=="-diagnose" set "PS_ARGS=-Diagnose"

if defined PS_ARGS (
    echo.
    echo Running diagnostic check...
    echo.
    powershell.exe -ExecutionPolicy Bypass -File "%~dp0scripts\windows-requirements.ps1" %PS_ARGS%
    echo.
    pause
    exit /b 0
)

echo.
echo This script will install development requirements for Plugable Chat.
echo Please keep this window open until the installation completes.
echo.
echo Tip: Run "requirements.bat --check" to diagnose issues without installing.
echo.

powershell.exe -ExecutionPolicy Bypass -File "%~dp0scripts\windows-requirements.ps1"

REM If we installed anything that requires a restart, pause so user sees the message
echo.
pause
