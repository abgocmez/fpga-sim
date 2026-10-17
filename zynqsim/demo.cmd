@echo off
rem ZynqSim demo: runs the curated tour (cargo xtask demo) from a plain terminal or a
rem double-click. Usage: demo.cmd [--quick] [--only name,...] [--gui]
rem The PATH lines make cargo and the MinGW linker visible when they are not on PATH.
setlocal
cd /d "%~dp0"
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
if exist "C:\Qt\Tools\mingw1310_64\bin\gcc.exe" set "PATH=C:\Qt\Tools\mingw1310_64\bin;%PATH%"
cargo xtask demo %*
set "RC=%ERRORLEVEL%"
echo.
if "%RC%"=="0" (echo demo finished: no FAIL) else (echo demo finished with FAILED steps, see the table above)
rem Without arguments (a double-click) keep the window open so the table can be read.
if "%~1"=="" pause
exit /b %RC%
