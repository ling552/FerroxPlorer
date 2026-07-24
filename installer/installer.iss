; FerroxPlorer Windows 安装程序脚本(Inno Setup 6)
; CI 中通过 /DAppVersion=x.y.z 传入版本号;本地手动编译时使用下方默认值
#ifndef AppVersion
  #define AppVersion "0.4.1"
#endif

[Setup]
; 固定 AppId 保证升级安装时覆盖旧版本而非并存
AppId={{B7E4F3A2-9C51-4D8E-A6B0-3F2D1E5C8A47}
AppName=FerroxPlorer
AppVersion={#AppVersion}
AppPublisher=ling552
AppPublisherURL=https://github.com/ling552/FerroxPlorer
AppSupportURL=https://github.com/ling552/FerroxPlorer/issues
AppUpdatesURL=https://github.com/ling552/FerroxPlorer/releases
DefaultDirName={autopf}\FerroxPlorer
DefaultGroupName=FerroxPlorer
UninstallDisplayIcon={app}\ferroxplorer.exe
OutputBaseFilename=FerroxPlorer-Setup-{#AppVersion}
OutputDir=Output
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
; 安装程序自身图标由根目录 icon.png 生成，需与应用图标同步更新
SetupIconFile=icon.ico
; 应用内更新场景:安装前自动关闭正在运行的 FerroxPlorer
CloseApplications=yes
; 默认装到 Program Files(需管理员);无管理员权限时允许降级装到用户目录
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\ferroxplorer.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\FerroxPlorer"; Filename: "{app}\ferroxplorer.exe"
Name: "{group}\{cm:UninstallProgram,FerroxPlorer}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\FerroxPlorer"; Filename: "{app}\ferroxplorer.exe"; Tasks: desktopicon

[Run]
; 安装完成后可勾选立即启动(应用内更新流程:安装结束直接回到新版本)
Filename: "{app}\ferroxplorer.exe"; Description: "{cm:LaunchProgram,FerroxPlorer}"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ResultCode: Integer;
begin
  if CurUninstallStep = usUninstall then
    Exec(ExpandConstant('{app}\ferroxplorer.exe'),
      '--unregister-default-file-manager', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
