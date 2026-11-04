import copy
import json
import unittest
from unittest.mock import patch
from pathlib import Path
import tempfile
import hashlib

import numpy as np
from PIL import Image, ImageCms

from worker import Pipeline, balance_neutral, make_cutout, parse_attributes


def attributes():
    return dict(is_clothing=True,name="Cream embroidered fleece",category="Outerwear",colors=["cream","burgundy"],
                warmth=3.5,formality=1.5,seasons=["autumn","winter"],fabric="fleece-like",pattern="plain",fit="relaxed",
                styles=["casual"],details=["Tree embroidery", "Quarter zip"],
                description="A cream fleece-like pullover with tree embroidery. The garment is laid flat on a carpeted floor.",
                uncertainties=["Fibre composition needs a label check"],bbox=[100,50,900,950],rotation_clockwise=90)


class WorkerTests(unittest.TestCase):
    def test_parse_preserves_real_decoration_and_removes_background_caption(self):
        result=parse_attributes(json.dumps(attributes()))
        self.assertIn("embroidery",result["description"])
        self.assertNotIn("carpet",result["description"])
        self.assertEqual(result["pattern"],"embroidery")
        self.assertEqual(result["warmth"],3.5)
        self.assertIn("embroidered",result["styles"])

    def test_template_echo_and_non_clothing_are_rejected(self):
        with self.assertRaisesRegex(ValueError,"repeated"):
            parse_attributes(json.dumps(dict(attributes(),colors=["main garment colour"])))
        with self.assertRaisesRegex(ValueError,"No single clothing"):
            parse_attributes(json.dumps(dict(attributes(),is_clothing=False)))

    def test_absence_of_embroidery_does_not_become_an_embroidery_tag(self):
        raw=dict(attributes(),description="Plain cotton shirt without embroidery.",details=["No embroidery or print"],styles=["casual"])
        result=parse_attributes(json.dumps(raw))
        self.assertEqual(result["pattern"],"plain")
        self.assertNotIn("embroidered",result["styles"])

    def test_invalid_model_fields_never_become_catalogue_facts(self):
        for change in [dict(warmth=8),dict(formality=0),dict(category="Anything"),dict(bbox=[900,0,100,1000]),dict(seasons=["monsoon"]),dict(colors=[])]:
            with self.subTest(change=change),self.assertRaises(ValueError):
                parse_attributes(json.dumps(dict(attributes(),**change)))

    def test_mask_crops_pads_and_preserves_actual_foreground_pixels(self):
        source=Image.new("RGBA",(200,160),(180,170,160,255))
        pixels=np.asarray(source).copy();pixels[40:120,60:140,:3]=[30,80,120];source=Image.fromarray(pixels)
        def segment(image):
            data=np.asarray(image).copy();data[:,:,3]=0;data[40:120,60:140,3]=255
            # A tiny detached foreground speck should not expand the crop.
            data[2:4,2:4,3]=255
            return Image.fromarray(data)
        result=make_cutout(source,[0,0,1000,1000],0,segment)
        self.assertEqual(result.size,(96,96));array=np.asarray(result)
        self.assertTrue((array[0,:,3]==0).all());self.assertTrue((array[:,0,3]==0).all())
        self.assertTrue((array[20,20,:3]==[30,80,120]).all());self.assertEqual(array[20,20,3],255)

    def test_empty_and_all_opaque_masks_are_rejected(self):
        source=Image.new("RGBA",(100,100),(80,90,100,255))
        with self.assertRaises(ValueError):make_cutout(source,[0,0,1000,1000],0,lambda im:Image.new("RGBA",im.size,(0,0,0,0)))
        with self.assertRaises(ValueError):make_cutout(source,[0,0,1000,1000],0,lambda im:im)

    def test_neutral_reference_corrects_cast_without_inventing_exposure(self):
        image=Image.new("RGBA",(100,100),(155,130,100,255))
        original=np.asarray(image).copy()
        corrected=np.asarray(balance_neutral(image,dict(x=0.5,y=0.5,radius=0.1)))
        self.assertLessEqual(int(corrected[50,50,:3].max())-int(corrected[50,50,:3].min()),1)
        self.assertTrue((corrected[:,:,3]==255).all());self.assertTrue((np.asarray(image)==original).all())

    def test_clipped_dark_nonuniform_or_invalid_reference_is_rejected(self):
        patch=dict(x=0.5,y=0.5,radius=0.1)
        for colour in [(255,255,255,255),(2,2,2,255),(120,120,120,0)]:
            with self.subTest(colour=colour),self.assertRaises(ValueError):
                balance_neutral(Image.new("RGBA",(100,100),colour),patch)
        with self.assertRaises(ValueError):balance_neutral(Image.new("RGBA",(100,100),(120,120,120,255)),dict(patch,x=1.5))
        data=np.full((100,100,4),255,dtype=np.uint8);data[:,:,:3]=100;data[::2,::2,:3]=220
        with self.assertRaisesRegex(ValueError,"not uniform"):balance_neutral(Image.fromarray(data),patch)

    def test_profile_orientation_and_reference_are_applied_before_recognition(self):
        test_root=Path(__file__).resolve().parent.parent/"test-data"/"python-tests"
        test_root.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(dir=test_root) as folder:
            # Verify the recursive TemporaryDirectory cleanup target is scoped
            # to this project's dedicated, generated test directory.
            self.assertEqual(Path(folder).resolve().parent,test_root.resolve())
            source=Path(folder)/"phone.jpg";output=Path(folder)/"cutout.png"
            image=Image.new("RGB",(80,40),(155,130,100));exif=Image.Exif();exif[274]=6
            profile=ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
            image.save(source,quality=98,exif=exif,icc_profile=profile)
            original=source.read_bytes();observed={}
            pipeline=Pipeline();pipeline.manifest={"revision":"test"};pipeline.segmenter=object()
            def inspect(image,request_id):
                observed["image"]=image.copy()
                return parse_attributes(json.dumps(attributes()))
            pipeline.infer=inspect
            # Isolate preprocessing: real model quality is checked separately on
            # actual garments through the Rust worker bridge.
            with patch("worker.make_cutout",lambda image,*args:image):
                result=pipeline.analyse(source,output,"unit",dict(x=0.5,y=0.5,radius=0.1))
            self.assertEqual(observed["image"].size,(40,80))
            pixel=observed["image"].getpixel((20,40))
            self.assertLessEqual(max(pixel)-min(pixel),2)
            self.assertEqual(source.read_bytes(),original)
            self.assertEqual(result["source_sha256"],hashlib.sha256(original).hexdigest())


if __name__=="__main__":unittest.main()
