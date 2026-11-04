"""Backend failure and asset validation checks; no GPU/model required."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

from gpu_backend import gpu_assets, parse_devices, create_segmenter
from gpu_setup import safe_extract, download, sha256
from worker import Pipeline


class GpuTests(unittest.TestCase):
    def setUp(self):
        root=Path(__file__).resolve().parent.parent/"test-data"/"python-tests"
        root.mkdir(parents=True,exist_ok=True)
        self.folder=tempfile.TemporaryDirectory(dir=root)
        self.root=Path(self.folder.name).resolve()
        self.assertEqual(self.root.parent,root.resolve())
        self.addCleanup(self.folder.cleanup)
        (self.root/"gpu-model.json").write_text(json.dumps(dict(schema=1,server="server.exe",model_file="model.gguf",projector_file="projector.gguf")))
        for name in ["server.exe","model.gguf","projector.gguf"]:(self.root/name).write_bytes(b"asset")

    def test_manifest_rejects_missing_assets_and_paths_outside_cache(self):
        self.assertTrue(gpu_assets(self.root)["server"].is_file())
        data=json.loads((self.root/"gpu-model.json").read_text())
        for value in ["../server.exe","missing.exe"]:
            data["server"]=value
            (self.root/"gpu-model.json").write_text(json.dumps(data))
            with self.assertRaises(ValueError):gpu_assets(self.root)

    def test_device_enumeration_keeps_real_adapter_names(self):
        devices=parse_devices("Available devices:\n  Vulkan0: AMD  Radeon RX 7800 XT (16368 MiB, 15000 MiB free)\n Vulkan1: Other (2048 MiB, 500 MiB free)")
        self.assertEqual(devices,[("Vulkan0","AMD  Radeon RX 7800 XT"),("Vulkan1","Other")])

    def test_auto_start_failure_retries_cpu_and_keeps_reason_but_gpu_is_strict(self):
        gpu=Mock();gpu.start.side_effect=RuntimeError("GPU busy")
        cpu=Mock(mode="cpu",device="CPU",runtime_version="test",manifest=dict(model_repo="test"))
        cpu.start.return_value=cpu
        with patch("worker.MODELS",self.root),patch("gpu_backend.LlamaVision",side_effect=[gpu,cpu]):
            pipeline=Pipeline("auto");pipeline.load("test")
            self.assertEqual(pipeline.recognition_backend,"llama.cpp CPU")
            self.assertEqual(pipeline.fallback_reasons,["GPU busy"])
            gpu.close.assert_called_once();pipeline.close()
        with patch("worker.MODELS",self.root),patch("gpu_backend.LlamaVision",return_value=gpu):
            with self.assertRaisesRegex(RuntimeError,"GPU busy"):Pipeline("gpu").load("test")

    def test_cpu_selection_does_not_try_gpu(self):
        cpu=Mock(mode="cpu",device="CPU",runtime_version="test",manifest=dict(model_repo="test"));cpu.start.return_value=cpu
        with patch("worker.MODELS",self.root),patch("gpu_backend.LlamaVision",return_value=cpu) as factory:
            pipeline=Pipeline("cpu");pipeline.load("test")
            self.assertEqual(factory.call_args.args[1],"cpu");pipeline.close()

    def test_directml_cannot_silently_report_cpu_as_gpu(self):
        import onnxruntime as ort
        session=Mock();session.inner_session.get_providers.return_value=["CPUExecutionProvider"]
        with patch.object(ort,"get_available_providers",return_value=["DmlExecutionProvider","CPUExecutionProvider"]),patch("rembg.sessions.birefnet_general_lite.BiRefNetSessionGeneralLite",return_value=session):
            with self.assertRaisesRegex(RuntimeError,"could not activate"):create_segmenter(self.root,"gpu")
        with patch.object(ort,"get_available_providers",return_value=["CPUExecutionProvider"]):
            with self.assertRaisesRegex(RuntimeError,"not installed"):create_segmenter(self.root,"gpu")

    def test_archive_rejects_traversal_before_extracting_any_member(self):
        archive=self.root/"runtime.zip"
        with zipfile.ZipFile(archive,"w") as bundle:
            bundle.writestr("legitimate.dll",b"ok");bundle.writestr("../escape.dll",b"bad")
        with self.assertRaisesRegex(ValueError,"unsafe path"):safe_extract(archive,self.root/"runtime")
        self.assertFalse((self.root/"runtime"/"legitimate.dll").exists())

    def test_verified_assets_skip_network_and_corrupt_download_is_not_published(self):
        target=self.root/"asset.bin";target.write_bytes(b"verified")
        with patch("requests.get") as get:
            download("https://example.invalid",target,sha256(target),8,lambda _:None);get.assert_not_called()
        response=Mock(status_code=200,headers={});response.iter_content.return_value=[b"corrupt"]
        with patch("requests.get",return_value=response):
            with self.assertRaisesRegex(ValueError,"Integrity"):download("https://example.invalid",target,"0"*64,7,lambda _:None)
        self.assertEqual(target.read_bytes(),b"verified")
        self.assertFalse(target.with_suffix(".bin.part").exists())

    def test_interrupted_download_resumes_and_validates_before_publish(self):
        import hashlib
        target=self.root/"resumed.bin";partial=target.with_suffix(".bin.part")
        partial.write_bytes(b"abc")
        response=Mock(status_code=206,headers={"Content-Range":"bytes 3-5/6"})
        response.iter_content.return_value=[b"def"]
        with patch("requests.get",return_value=response) as get:
            download("https://example.invalid",target,hashlib.sha256(b"abcdef").hexdigest(),6,lambda _:None)
            self.assertEqual(get.call_args.kwargs["headers"],{"Range":"bytes=3-"})
        self.assertEqual(target.read_bytes(),b"abcdef")


if __name__=="__main__":unittest.main()
