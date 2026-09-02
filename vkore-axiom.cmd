@echo off
setlocal
for /f "usebackq delims=" %%I in (`wsl.exe --exec wslpath -a "%~dp0."`) do set "VKORE_WSL_ROOT=%%I"
if not defined VKORE_WSL_ROOT (
  echo Could not translate the vKore repository path for WSL. 1>&2
  exit /b 2
)
wsl.exe --exec bash "%VKORE_WSL_ROOT%/vkore-axiom" %*
exit /b %ERRORLEVEL%
