#!/usr/bin/env python3
"""Reuse exact completed Android native artifacts for instrumented test retries.

Authenticates the original run/artifact, checks container/APK/native hashes,
actual manifest and ELF ABIs, then copies libraries without cross-compilation.
No signing key, account credentials, native ABI bypass or runtime verdict.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import urllib.error
import urllib.request
import zipfile


class Rejected(ValueError):
    pass


def require(value, code):
    if not value:
        raise Rejected(code)


def manifest(data):
 strings=[];offset=8
 def length(p):
  a=data[p];return (((a&127)<<8)|data[p+1],p+2) if a&128 else (a,p+1)
 while offset+8<=len(data):
  kind,header,size=struct.unpack_from('<HHI',data,offset);assert size>=header>=8 and offset+size<=len(data)
  if kind==1:
   count,styles,flags,start,style_start=struct.unpack_from('<IIIII',data,offset+8)
   for i in range(count):
    p=offset+start+struct.unpack_from('<I',data,offset+header+4*i)[0]
    if flags&256:
     _,p=length(p);n,p=length(p);strings.append(data[p:p+n].decode())
    else:
     n=struct.unpack_from('<H',data,p)[0];p+=2
     if n&32768:n=((n&32767)<<16)|struct.unpack_from('<H',data,p)[0];p+=2
     strings.append(data[p:p+2*n].decode('utf-16le'))
  if kind==0x102:
   name=struct.unpack_from('<I',data,offset+20)[0]
   if strings[name]=='manifest':
    start,width,count=struct.unpack_from('<HHH',data,offset+24);result={}
    for i in range(count):
     p=offset+16+start+width*i;name,raw=struct.unpack_from('<II',data,p+4);value_type=data[p+15];value=struct.unpack_from('<I',data,p+16)[0]
     key=strings[name]
     if key in ['package','versionCode','versionName']:
      result[key]=strings[raw] if raw!=0xffffffff else strings[value] if value_type==3 else value
    return result
  offset+=size
 raise ValueError('manifest root not found')


def validate_apk(apk, pin, origin):
    require(hashlib.file_digest(apk.open('rb'),'sha256').hexdigest()==pin['apk_sha256'], 'APK_HASH_MISMATCH')
    require(apk.stat().st_size==pin['apk_bytes'], 'APK_SIZE_MISMATCH')
    expected={row['path']:row for row in pin['native_libraries']}
    paths={f'lib/{abi}/{name}.so' for abi in ('arm64-v8a','x86_64') for name in ('libppvpn_flutter','libppvpn_tunnel')}
    require(set(expected)==paths, 'EXACT_FOUR_NATIVE_LIBRARIES_REQUIRED')
    payloads={}
    with zipfile.ZipFile(apk) as archive:
        require(len(archive.namelist())==len(set(archive.namelist())), 'DUPLICATE_APK_MEMBER')
        info=manifest(archive.read('AndroidManifest.xml'))
        require(info==pin['manifest'], 'ACTUAL_ANDROID_MANIFEST_DIFFERS')
        require(info['package']=='com.peakpassvpn.ppvpn', 'WRONG_APPLICATION_ID')
        for name in sorted(paths):
            data=archive.read(name)
            require(len(data)==expected[name]['bytes'] and hashlib.sha256(data).hexdigest()==expected[name]['sha256'], 'NATIVE_LIBRARY_HASH_MISMATCH')
            machine=183 if '/arm64-v8a/' in name else 62
            require(data[:6]==b'\x7fELF\x02\x01' and struct.unpack_from('<H',data,18)[0]==machine, 'ELF_ABI_MISMATCH')
            if name.endswith('/libppvpn_flutter.so'):
                require(origin.encode() in data, 'COMPILED_API_ORIGIN_LITERAL_MISSING')
            payloads[name]=data
        for abi in ('arm64-v8a','x86_64'):
            name=f'lib/{abi}/libc++_shared.so'
            if name in archive.namelist():
                data=archive.read(name)
                require(data[:6]==b'\x7fELF\x02\x01' and struct.unpack_from('<H',data,18)[0]==(183 if abi=='arm64-v8a' else 62), 'CPP_RUNTIME_ABI_MISMATCH')
                payloads[name]=data
    return info,payloads


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):
        return None


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pin',type=Path,required=True)
    parser.add_argument('--product-root',type=Path,required=True)
    parser.add_argument('--origin',required=True)
    parser.add_argument('--work',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    try:
        pin=json.loads(args.pin.read_text())
        product=args.product_root.resolve(strict=True)
        require(subprocess.check_output(['git','rev-parse','HEAD'],cwd=product,text=True).strip()==pin['product_source'], 'PRODUCT_SOURCE_MISMATCH')
        require(hashlib.sha256(args.origin.encode()).hexdigest()==pin['backend_origin_sha256'], 'ORIGIN_PIN_MISMATCH')
        repository=os.environ['GITHUB_REPOSITORY']
        require(repository=='peakpassvpn/ppvpn-client', 'WRONG_REPOSITORY')
        token=os.environ['GH_TOKEN']
        base=f'https://api.github.com/repos/{repository}/actions'
        def api(path):
            request=urllib.request.Request(base+path,headers={'Authorization':'Bearer '+token,'Accept':'application/vnd.github+json'})
            with urllib.request.urlopen(request,timeout=30) as response:return json.load(response)
        run=api(f"/runs/{int(pin['run'])}")
        require(run['head_sha']==pin['product_source'] and run['status']=='completed', 'ORIGINAL_RUN_SOURCE_OR_STATUS_MISMATCH')
        jobs=api(f"/runs/{int(pin['run'])}/jobs?per_page=100")['jobs']
        android=next(job for job in jobs if job['name']=='android')
        passed={step['name'] for step in android['steps'] if step.get('conclusion')=='success'}
        require({'Build','Native tunnel lifecycle'}<=passed, 'ORIGINAL_NATIVE_BUILD_OR_LIFECYCLE_DID_NOT_PASS')
        metadata=api(f"/artifacts/{int(pin['artifact_id'])}")
        require(metadata['name']=='flutter-apk-android' and not metadata['expired'], 'WRONG_OR_EXPIRED_ARTIFACT')
        require(metadata['workflow_run']['id']==pin['run'] and metadata['workflow_run']['head_sha']==pin['product_source'], 'ARTIFACT_SOURCE_MISMATCH')
        require(metadata['digest']=='sha256:'+pin['artifact_zip_sha256'], 'SERVER_ARTIFACT_DIGEST_MISMATCH')
        args.work.mkdir(parents=True,exist_ok=False)
        request=urllib.request.Request(base+f"/artifacts/{int(pin['artifact_id'])}/zip",headers={'Authorization':'Bearer '+token})
        try:
            urllib.request.build_opener(NoRedirect()).open(request,timeout=30)
            raise Rejected('ARTIFACT_DOWNLOAD_REDIRECT_MISSING')
        except urllib.error.HTTPError as error:
            require(error.code in (301,302,303,307), 'ARTIFACT_DOWNLOAD_REJECTED')
            location=error.headers['Location']
        require(location.startswith('https://'), 'ARTIFACT_DOWNLOAD_MUST_USE_TLS')
        downloaded=args.work/'runtime.zip'
        # Do not send the repository token to signed artifact storage.
        with urllib.request.urlopen(location,timeout=60) as response, downloaded.open('xb') as target:
            while chunk:=response.read(1024*1024):target.write(chunk)
        require(hashlib.file_digest(downloaded.open('rb'),'sha256').hexdigest()==pin['artifact_zip_sha256'], 'DOWNLOADED_ARTIFACT_HASH_MISMATCH')
        apk=args.work/'app-release.apk'
        with zipfile.ZipFile(downloaded) as archive:
            items=[item for item in archive.infolist() if item.filename.endswith('.apk')]
            require(len(items)==1, 'ONE_ORIGINAL_APK_REQUIRED')
            apk.write_bytes(archive.read(items[0]))
        info,payloads=validate_apk(apk,pin,args.origin)
        destination=product/'apps/flutter/android/app/src/main/jniLibs'
        require((product/'apps/flutter/android/app/src/main/AndroidManifest.xml').is_file(), 'GENERATED_RUNNER_MISSING')
        copied=[]
        for name,data in payloads.items():
            _,abi,filename=name.split('/')
            path=destination/abi/filename
            require(not path.exists(), 'NATIVE_DESTINATION_MUST_BE_EMPTY')
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_bytes(data)
            copied.append({'path':name,'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()})
        report={'product_source':pin['product_source'],'original_run':pin['run'],'artifact_id':pin['artifact_id'],
                'original_run_overall_conclusion':run['conclusion'],'original_build_and_native_lifecycle_passed':True,
                'artifact_zip_sha256':pin['artifact_zip_sha256'],'apk_sha256':pin['apk_sha256'],'manifest':info,
                'backend_origin_sha256':pin['backend_origin_sha256'],'native_libraries_reused':copied,
                'android_rust_jni_cross_build_executed':False,'forwarding_runtime_verdict':'NOT ASSESSED by reuse',
                'scope':'Exact Release native libraries for a separate instrumented Flutter integration test; native FRB initialization/hash checks remain enabled'}
        args.output.write_text(json.dumps(report,indent=2)+'\n')
        print('Verified exact original native libraries copied; no Android native cross-build performed')
    except Rejected as error:
        parser.exit(1,'Verified runtime reuse rejected: '+str(error)+'\n')
    except Exception:
        parser.exit(1,'Verified runtime reuse failed; no raw token/download diagnostics exported\n')


if __name__=='__main__':
    main()
