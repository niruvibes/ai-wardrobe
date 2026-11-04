"""Pinned, checksum-verified GPU assets from the upstream release/model owners."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import time
import zipfile

ROOT=Path(__file__).resolve().parent.parent
MODELS=Path(os.environ.get("WARDROBE_MODELS_DIR",ROOT/"models"))
RUNTIME_TAG="b12102"
RUNTIME_SHA="b322f7d6ce8b2a31cf16980aa38986a6a27da2e55e406e770f0221621c38de08"
RUNTIME_URL=f"https://github.com/ggml-org/llama.cpp/releases/download/{RUNTIME_TAG}/llama-{RUNTIME_TAG}-bin-win-vulkan-x64.zip"
MODEL_REPO="Qwen/Qwen3-VL-2B-Instruct-GGUF"
MODEL_REVISION="52d6c8ffea26cc873ac5ad116f8631268d7eb503"
FILES={
    "Qwen3VL-2B-Instruct-F16.gguf":("27b80f2e258f8372c9ee3a9b2293bd747cc08b18ed99709b57288cf457c42595",3447350304),
    "mmproj-Qwen3VL-2B-Instruct-F16.gguf":("c3d5afbef5287953acd57b4043d2269456e5761a4eaccb3b71b062996970aea5",819394848),
}

def sha256(path):
    digest=hashlib.sha256()
    with Path(path).open("rb") as file:
        for chunk in iter(lambda:file.read(8*1024*1024),b""):digest.update(chunk)
    return digest.hexdigest()

def download(url,path,expected_hash,expected_size,notify):
    import requests
    path=Path(path);path.parent.mkdir(parents=True,exist_ok=True)
    if path.is_file() and path.stat().st_size==expected_size and sha256(path)==expected_hash:
        notify(f"Verified {path.name}.");return
    partial=path.with_suffix(path.suffix+".part")
    offset=partial.stat().st_size if partial.exists() else 0
    if offset>=expected_size:
        partial.unlink();offset=0
    response=requests.get(url,headers={"Range":f"bytes={offset}-"} if offset else {},stream=True,timeout=(30,120))
    response.raise_for_status()
    if offset and (response.status_code!=206 or not response.headers.get("Content-Range","").startswith(f"bytes {offset}-")):
        offset=0
    written=offset;last=0.0
    with partial.open("ab" if offset else "wb") as file:
        for chunk in response.iter_content(2*1024*1024):
            file.write(chunk);written+=len(chunk)
            if time.monotonic()-last>2:
                notify(f"Downloading {path.name}: {written/1024**2:.0f} / {expected_size/1024**2:.0f} MB");last=time.monotonic()
    response.close()
    if partial.stat().st_size!=expected_size or sha256(partial)!=expected_hash:
        partial.unlink()
        raise ValueError(f"Integrity check failed for {path.name}. Retry setup for a fresh download.")
    partial.replace(path)

def safe_extract(archive,destination):
    destination=Path(destination).resolve();destination.mkdir(parents=True,exist_ok=True)
    with zipfile.ZipFile(archive) as bundle:
        for member in bundle.infolist():
            target=(destination/member.filename).resolve()
            if not target.is_relative_to(destination) or stat.S_ISLNK(member.external_attr>>16):
                raise ValueError("The runtime archive contains an unsafe path.")
        bundle.extractall(destination)

def setup_gpu(notify=print):
    if os.name!="nt":raise ValueError("The packaged Vulkan backend currently targets Windows x64.")
    folder=MODELS/"gpu";folder.mkdir(parents=True,exist_ok=True)
    archive=folder/f"llama-{RUNTIME_TAG}-vulkan.zip"
    download(RUNTIME_URL,archive,RUNTIME_SHA,37660949,notify)
    runtime=folder/f"runtime-{RUNTIME_TAG}"
    marker=runtime/"wardrobe-runtime.json"
    if not marker.exists() or json.loads(marker.read_text("utf-8")).get("archive_sha256")!=RUNTIME_SHA:
        safe_extract(archive,runtime)
        marker.write_text(json.dumps({"archive_sha256":RUNTIME_SHA,"release":RUNTIME_TAG}),"utf-8")
    for name,(digest,size) in FILES.items():
        download(f"https://huggingface.co/{MODEL_REPO}/resolve/{MODEL_REVISION}/{name}",folder/name,digest,size,notify)
    executables=list(runtime.rglob("llama-server.exe"))
    if len(executables)!=1:raise ValueError("The GPU runtime archive does not contain exactly one inference server.")
    manifest={"schema":1,"runtime_tag":RUNTIME_TAG,"runtime_sha256":RUNTIME_SHA,
              "server":str(executables[0].relative_to(MODELS)),"model_repo":MODEL_REPO,"revision":MODEL_REVISION,
              "model_file":f"gpu/{next(n for n in FILES if not n.startswith('mmproj'))}",
              "projector_file":f"gpu/{next(n for n in FILES if n.startswith('mmproj'))}",
              "files":{name:{"sha256":digest,"size":size} for name,(digest,size) in FILES.items()}}
    temporary=MODELS/"gpu-model.json.tmp";temporary.write_text(json.dumps(manifest,indent=2),"utf-8");temporary.replace(MODELS/"gpu-model.json")
    notify("Vulkan runtime and full-precision clothing model are ready.")
    return manifest

if __name__=="__main__":
    argparse.ArgumentParser(description=__doc__).parse_args()
    print(json.dumps(setup_gpu(),indent=2))
