"""Private local Vulkan inference and DirectML masking. No remote image calls."""
from __future__ import annotations
import atexit
import base64
import io
import json
import os
from pathlib import Path
import re
import secrets
import socket
import subprocess
import time


def hidden_process_options():
    return {"creationflags":subprocess.CREATE_NO_WINDOW} if os.name=="nt" else {}


def gpu_assets(models):
    models=Path(models).resolve()
    manifest=json.loads((models/"gpu-model.json").read_text("utf-8"))
    if manifest.get("schema")!=1:raise ValueError("Unsupported GPU model manifest. Run photo setup again.")
    for key in ["server","model_file","projector_file"]:
        path=(models/manifest[key]).resolve()
        if not path.is_relative_to(models) or not path.is_file():raise ValueError("A local GPU model file is missing. Run photo setup again.")
        manifest[key]=path
    return manifest


def parse_devices(text):
    return [(device,name.strip()) for device,name in re.findall(r"^\s*(Vulkan\d+):\s*(.*?)\s*\(\d+ MiB",text,re.M)]


def list_devices(server):
    result=subprocess.run([str(server),"--list-devices"],capture_output=True,text=True,encoding="utf-8",errors="replace",timeout=30,**hidden_process_options())
    if result.returncode:raise RuntimeError("The Vulkan driver could not enumerate the GPU. Check your Radeon driver and restart the app.")
    return parse_devices(result.stdout+"\n"+result.stderr)


class LlamaVision:
    def __init__(self,models,mode="gpu",notify=lambda message:None):
        if mode not in ["gpu","cpu"]:raise ValueError("Choose GPU or CPU inference.")
        self.manifest=gpu_assets(models);self.mode=mode;self.notify=notify
        self.child=None;self.http=None;self.address=None;self.token=None;self.gpu_layers=0
        self.device="CPU";self.runtime_version=self.manifest["runtime_tag"]
        log_dir=Path(os.environ.get("WARDROBE_LOG_DIR",Path(models).parent/"test-data"/"gpu-logs"))
        log_dir.mkdir(parents=True,exist_ok=True)
        self.log_path=log_dir/f"vision-vulkan-{os.getpid()}-{secrets.token_hex(4)}.log"
        atexit.register(self.close)

    def start(self):
        import requests
        selected="none"
        if self.mode=="gpu":
            devices=list_devices(self.manifest["server"])
            if not devices:raise RuntimeError("No Vulkan GPU was found. Choose Auto/CPU or check the Radeon driver.")
            # Prefer the requested Radeon, then another discrete-looking adapter.
            selected,name=next((d for d in devices if "7800 XT" in d[1]),devices[0])
            self.device=f"{selected} · {name}"
        self.notify(f"Loading clothing recognition on {self.device}…")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1",0));port=listener.getsockname()[1]
        self.token=secrets.token_urlsafe(32)
        command=[str(self.manifest["server"]),"-m",str(self.manifest["model_file"]),"--mmproj",str(self.manifest["projector_file"]),
                 "--host","127.0.0.1","--port",str(port),"--api-key",self.token,"--no-webui",
                 "--ctx-size","4096","--parallel","1","--image-min-tokens","1024","--image-max-tokens","1280",
                 "--device",selected,"--n-gpu-layers","999" if self.mode=="gpu" else "0",
                 "--threads",str(min(8,max(1,(os.cpu_count() or 4)//2))),"--flash-attn","off","-lv","4"]
        if self.mode=="cpu":command.append("--no-mmproj-offload")
        self._log=self.log_path.open("ab",buffering=0)
        self.child=subprocess.Popen(command,stdin=subprocess.DEVNULL,stdout=self._log,stderr=self._log,
                                    cwd=self.manifest["server"].parent,**hidden_process_options())
        self.http=requests.Session()
        # Never route clothing photos through a machine-wide HTTP proxy.
        self.http.trust_env=False
        self.http.headers["Authorization"]="Bearer "+self.token
        self.address=f"http://127.0.0.1:{port}"
        deadline=time.monotonic()+180
        while time.monotonic()<deadline:
            if self.child.poll() is not None:
                self.close();raise RuntimeError(f"The local {self.mode.upper()} model could not start. Details: {self.log_path.name}")
            try:
                response=self.http.get(self.address+"/health",timeout=1)
                if response.ok:break
            except requests.RequestException as error:
                if "10013" in str(error):
                    self.close();raise RuntimeError("Local GPU helper communication is blocked by the current process permissions.") from error
            time.sleep(0.1)
        else:
            self.close();raise RuntimeError("Loading the local vision model timed out.")
        if self.mode=="gpu":
            logs=self.log_path.read_text("utf-8",errors="replace")
            offload=re.search(r"offloaded (\d+)/(\d+) layers to GPU",logs)
            self.gpu_layers=int(offload[1]) if offload else 0
            if self.gpu_layers==0:
                self.close();raise RuntimeError("The model did not offload any layers to the GPU. Choose Auto/CPU or check the GPU log.")
        return self

    def infer(self,image,prompt):
        if self.child is None or self.child.poll() is not None:raise RuntimeError("The local inference process has stopped.")
        image=image.convert("RGB").copy();image.thumbnail((1280,1280))
        buffer=io.BytesIO();image.save(buffer,"PNG")
        photo="data:image/png;base64,"+base64.b64encode(buffer.getvalue()).decode("ascii")
        payload={"model":"wardrobe-vision","messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":photo}},{"type":"text","text":prompt}]}],
                 "temperature":0,"max_tokens":768,"stream":False,"response_format":{"type":"json_object"}}
        response=self.http.post(self.address+"/v1/chat/completions",json=payload,timeout=(5,600))
        if not response.ok:raise RuntimeError(f"Local GPU inference failed (HTTP {response.status_code}); see {self.log_path.name}.")
        result=response.json()
        content=result["choices"][0]["message"]["content"]
        if not isinstance(content,str):raise RuntimeError("The local model returned no clothing description.")
        return content,result.get("timings",{})

    def close(self):
        if self.child is not None:
            if self.child.poll() is None:
                self.child.terminate()
                try:self.child.wait(timeout=5)
                except subprocess.TimeoutExpired:self.child.kill();self.child.wait()
            self.child=None
        if self.http is not None:self.http.close();self.http=None
        if getattr(self,"_log",None) is not None:self._log.close();self._log=None


def create_segmenter(models,mode="auto",notify=lambda message:None):
    import onnxruntime as ort
    from rembg.sessions.birefnet_general_lite import BiRefNetSessionGeneralLite
    os.environ["U2NET_HOME"]=str(Path(models)/"rembg")
    available=ort.get_available_providers()
    gpu=mode!="cpu" and "DmlExecutionProvider" in available
    if mode=="gpu" and not gpu:raise RuntimeError("DirectML is not installed in this environment. Run GPU photo setup.")
    options=ort.SessionOptions()
    options.enable_mem_pattern=False
    options.execution_mode=ort.ExecutionMode.ORT_SEQUENTIAL
    options.intra_op_num_threads=min(4,os.cpu_count() or 4)
    providers=[("DmlExecutionProvider",{"device_id":0}),"CPUExecutionProvider"] if gpu else ["CPUExecutionProvider"]
    notify("Loading background removal on DirectML…" if gpu else "Loading background removal on CPU…")
    session=BiRefNetSessionGeneralLite("birefnet-general-lite",options,providers=providers)
    active=session.inner_session.get_providers()
    if gpu and (not active or active[0]!="DmlExecutionProvider"):
        raise RuntimeError("DirectML could not activate the GPU for background removal.")
    return session,"DirectML" if gpu else "CPU"
