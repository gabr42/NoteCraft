@echo off
rem Builds NoteCraft.exe for copying to other computers and puts it in the dist folder.
cd /d "%~dp0"
rem cargo lives in the user's .cargo\bin, which a plain command prompt may not have on its PATH.
set "PATH=%PATH%;%USERPROFILE%\.cargo\bin"
cargo build --release || exit /b 1
if not exist dist mkdir dist
copy /y target\release\notecraft.exe dist\NoteCraft.exe >nul || exit /b 1
echo Done: %~dp0dist\NoteCraft.exe
