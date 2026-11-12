"""Check actual package versions against the frozen workspace input."""
import pathlib,plistlib,re,subprocess,tomllib

def workspace_version(source,env):
    value=tomllib.loads((source/'Cargo.toml').read_text())['workspace']['package']['version']
    assert re.fullmatch(r'\d+\.\d+\.\d+',value)
    assert env.get('PPVPN_VERSION') in (None,'',value),'Version override differs from frozen workspace'
    env['PPVPN_VERSION']=value
    return value

def apple_versions(app,version,build,ios=False):
    paths=[app/'Info.plist'] if ios else [app/'Contents/Info.plist']
    if ios:paths.append(app/'PlugIns/PacketTunnel.appex/Info.plist')
    result=[]
    for path in paths:
        info=plistlib.loads(path.read_bytes())
        assert info['CFBundleShortVersionString']==version,'Actual Apple product version differs'
        assert str(info['CFBundleVersion'])==str(build),'Actual Apple build differs'
        result.append(dict(bundle=info['CFBundleIdentifier'],version=info['CFBundleShortVersionString'],build=str(info['CFBundleVersion'])))
    return result

def android_versions(candidate,version,build,env):
    sdk=pathlib.Path(env.get('ANDROID_HOME') or env['ANDROID_SDK_ROOT'])
    tools=sorted((sdk/'build-tools').glob('*/aapt'))
    assert tools,'Actual Android package version verification requires aapt'
    result=[]
    for path in sorted(candidate.glob('*.apk')):
        raw=subprocess.check_output([str(tools[-1]),'dump','badging',str(path)],env=env,text=True)
        match=re.search(r"^package: name='([^']+)' versionCode='([^']+)' versionName='([^']+)'",raw,re.M)
        assert match and match[3]==version,'Actual APK version differs'
        if path.name=='app-release.apk':assert match[2]==str(build),'Actual universal APK build differs'
        result.append(dict(package=path.name,application_id=match[1],version=match[3],version_code=match[2]))
    assert any(row['package']=='app-release.apk' for row in result)
    return result
