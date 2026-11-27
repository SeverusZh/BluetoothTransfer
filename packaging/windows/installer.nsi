; 蓝牙传输 Windows 安装器：per-user，无需管理员，只写 HKCU 与本应用目录。
Unicode true
SetCompressor /SOLID lzma
SetCompressorDictSize 64

!include "MUI2.nsh"

!ifndef VERSION
  !error "VERSION 未定义：使用 /DVERSION=<版本> 传入，版本号以 workspace.package.version 为唯一来源"
!endif
!ifndef BUILD_DIR
  !define BUILD_DIR "../../target/release"
!endif
!ifndef OUTPUT_FILE
  !define OUTPUT_FILE "../../dist/BluetoothTransfer-${VERSION}-setup.exe"
!endif

!define APP_NAME "蓝牙传输"
!define APP_EXE "bt-gui.exe"
!define PUBLISHER "BlueToothTransfer"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlueToothTransfer"
!define APP_KEY "Software\BlueToothTransfer"

Name "${APP_NAME} ${VERSION}"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\BlueToothTransfer"
InstallDirRegKey HKCU "${APP_KEY}" "InstallDir"
RequestExecutionLevel user
ShowInstDetails show
ShowUninstDetails show
BrandingText "${APP_NAME}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=2052 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=2052 "FileDescription" "${APP_NAME} 安装器"
VIAddVersionKey /LANG=2052 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "MIT License"

!define MUI_ICON "${__FILEDIR__}\app.ico"
!define MUI_UNICON "${__FILEDIR__}\app.ico"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

LangString DESC_MAIN ${LANG_SIMPCHINESE} "主程序、命令行工具与开始菜单快捷方式，必需。"
LangString DESC_SHELL ${LANG_SIMPCHINESE} "在文件、文件夹和 .txt 右键菜单加入发送入口，并创建「发送到」快捷方式；只写当前用户注册表。"
LangString DESC_AUTOSTART ${LANG_SIMPCHINESE} "登录 Windows 后自动启动并最小化到托盘，可随时在设置页关闭。"

Section "主程序（必需）" SEC_MAIN
  SectionIn RO
  SetOverwrite on
  SetOutPath "$INSTDIR"
  ; 安装前结束本应用进程，避免覆盖运行中的 exe。
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM ${APP_EXE} /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM btrecv.exe /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM bt.exe /F'
  Pop $0
  ; 先清理本应用旧的集成项，重复安装或改选组件时不会残留。
  DeleteRegKey HKCU "Software\Classes\*\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\Drive\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "BluetoothTransfer"
  Delete "$APPDATA\Microsoft\Windows\SendTo\${APP_NAME}.lnk"
  File "/oname=${APP_EXE}" "${BUILD_DIR}/${APP_EXE}"
  !if /FileExists "${BUILD_DIR}/bt.exe"
    File "/oname=bt.exe" "${BUILD_DIR}/bt.exe"
  !endif
  !if /FileExists "${BUILD_DIR}/btrecv.exe"
    File "/oname=btrecv.exe" "${BUILD_DIR}/btrecv.exe"
  !endif
  !if /FileExists "${BUILD_DIR}/../../LICENSE"
    File "/oname=LICENSE.txt" "${BUILD_DIR}/../../LICENSE"
  !endif
  Delete "$INSTDIR\uninstall.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载.lnk" "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "${APP_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE},0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1

SectionEnd

Section /o "资源管理器右键菜单" SEC_SHELL
  WriteRegStr HKCU "Software\Classes\*\shell\BluetoothTransfer.Send" "" "用蓝牙传输发送"
  WriteRegStr HKCU "Software\Classes\*\shell\BluetoothTransfer.Send" "MUIVerb" "用蓝牙传输发送"
  WriteRegStr HKCU "Software\Classes\*\shell\BluetoothTransfer.Send" "Icon" '"$INSTDIR\${APP_EXE}",0'
  WriteRegStr HKCU "Software\Classes\*\shell\BluetoothTransfer.Send" "MultiSelectModel" "Single"
  WriteRegStr HKCU "Software\Classes\*\shell\BluetoothTransfer.Send" "Position" "Top"
  WriteRegStr HKCU "Software\Classes\*\shell\BluetoothTransfer.Send\command" "" '"$INSTDIR\${APP_EXE}" --send "%1"'

  WriteRegStr HKCU "Software\Classes\Directory\shell\BluetoothTransfer.Send" "" "用蓝牙传输发送"
  WriteRegStr HKCU "Software\Classes\Directory\shell\BluetoothTransfer.Send" "MUIVerb" "用蓝牙传输发送"
  WriteRegStr HKCU "Software\Classes\Directory\shell\BluetoothTransfer.Send" "Icon" '"$INSTDIR\${APP_EXE}",0'
  WriteRegStr HKCU "Software\Classes\Directory\shell\BluetoothTransfer.Send" "MultiSelectModel" "Single"
  WriteRegStr HKCU "Software\Classes\Directory\shell\BluetoothTransfer.Send\command" "" '"$INSTDIR\${APP_EXE}" --send "%1"'

  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\BluetoothTransfer.Send" "" "用蓝牙传输发送当前文件夹"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\BluetoothTransfer.Send" "MUIVerb" "用蓝牙传输发送当前文件夹"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\BluetoothTransfer.Send" "Icon" '"$INSTDIR\${APP_EXE}",0'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\BluetoothTransfer.Send\command" "" '"$INSTDIR\${APP_EXE}" --send "%V"'

  WriteRegStr HKCU "Software\Classes\Drive\shell\BluetoothTransfer.Send" "" "用蓝牙传输发送此驱动器"
  WriteRegStr HKCU "Software\Classes\Drive\shell\BluetoothTransfer.Send" "MUIVerb" "用蓝牙传输发送此驱动器"
  WriteRegStr HKCU "Software\Classes\Drive\shell\BluetoothTransfer.Send" "Icon" '"$INSTDIR\${APP_EXE}",0'
  WriteRegStr HKCU "Software\Classes\Drive\shell\BluetoothTransfer.Send\command" "" '"$INSTDIR\${APP_EXE}" --send "%1"'

  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText" "" "用蓝牙发送文本内容"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText" "MUIVerb" "用蓝牙发送文本内容"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText" "Icon" '"$INSTDIR\${APP_EXE}",0'
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText\command" "" '"$INSTDIR\${APP_EXE}" --send-text "%1"'

  CreateShortCut "$APPDATA\Microsoft\Windows\SendTo\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "--send" "$INSTDIR\${APP_EXE}" 0
SectionEnd

Section /o "开机自启" SEC_AUTOSTART
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "BluetoothTransfer" '"$INSTDIR\${APP_EXE}" --minimized'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_MAIN} $(DESC_MAIN)
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_SHELL} $(DESC_SHELL)
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_AUTOSTART} $(DESC_AUTOSTART)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function .onInit
  SetShellVarContext current
  ; 升级时沿用上次的组件选择，避免右键菜单或自启被静默移除。
  ReadRegStr $0 HKCU "Software\Classes\*\shell\BluetoothTransfer.Send" ""
  StrCmp $0 "" +2
  SectionSetFlags ${SEC_SHELL} ${SF_SELECTED}
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "BluetoothTransfer"
  StrCmp $0 "" +2
  SectionSetFlags ${SEC_AUTOSTART} ${SF_SELECTED}
FunctionEnd

Section "Uninstall"
  SetShellVarContext current
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM ${APP_EXE} /F'
  Pop $0
  IfSilent keep_data
  MessageBox MB_YESNO|MB_ICONQUESTION "是否同时删除用户数据（传输记录、配置与日志）？选择「否」将保留这些数据以便重新安装后继续使用。" IDNO keep_data
    RMDir /r "$LOCALAPPDATA\BlueToothTransfer"
    RMDir /r "$APPDATA\BlueToothTransfer"
  keep_data:

  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$APPDATA\Microsoft\Windows\SendTo\${APP_NAME}.lnk"

  DeleteRegKey HKCU "Software\Classes\*\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\Drive\shell\BluetoothTransfer.Send"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "BluetoothTransfer"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "${APP_KEY}"

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\bt.exe"
  Delete "$INSTDIR\btrecv.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd