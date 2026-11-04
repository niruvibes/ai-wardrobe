"""Measure cold and warm end-to-end imports, saving actual backend evidence."""
import argparse
import contextlib
import json
from pathlib import Path
import sys
import time

from worker import Pipeline

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("photos",nargs="+",type=Path)
    parser.add_argument("--output",type=Path,default=Path("test-data/gpu-benchmark"))
    parser.add_argument("--modes",nargs="+",choices=["auto","gpu","cpu"],default=["auto","gpu","cpu"])
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    report=[]
    for mode in args.modes:
        pipeline=Pipeline(mode)
        try:
            for index,source in enumerate(args.photos):
                started=time.monotonic();output=args.output/f"{mode}-{index}.png"
                result=pipeline.analyse(source,output,f"{mode}-{index}")
                elapsed=round(time.monotonic()-started,3)
                result.update(mode=mode,elapsed_seconds=elapsed,output=str(output),sample=index)
                report.append(result)
                (args.output/"benchmark.json").write_text(json.dumps(report,indent=2),"utf-8")
                print(json.dumps({"mode":mode,"sample":index,"seconds":elapsed,"recognition":result["recognition_backend"],"segmentation":result["segmentation_backend"],"gpu_layers":result["gpu_layers"],"name":result["attributes"]["name"],"timings":result["timings"]}),file=sys.stderr,flush=True)
        finally:pipeline.close()

if __name__=="__main__":
    with contextlib.redirect_stdout(sys.stderr):main()
