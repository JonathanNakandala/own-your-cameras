# AI detection (person, vehicle, pet)

What the stock firmware does for the paw icon, from the extracted PAK. Static analysis only: nothing here has
been run on a camera, and no code in `cam` uses it.

## What runs

- Person, vehicle and dog/cat detection is a neural network run on the SoC's AI unit. It is separate from
  the motion engine described in [motion-detection.md](motion-detection.md).
- The stock `device` binary calls `av_ppvdcnn_get_result` per processed frame (`Na_ai_process`). It logs boxes
  as `person|VEHICLE|dog_cat cnt … x_min y_min x_max y_max`, then raises `MSG_CFG_PERSON_DETECT_NOTIFY`,
  `MSG_CFG_VEHICLE_DETECT_NOTIFY` and `MSG_CFG_DOG_CAT_DETECT_NOTIFY`. Whether pets are offered is gated by
  `support_ai_animal_type`.
- The detector classes are `BC_PPVD_V3`, `Bc_PPVD_Lite`, `BC_PPVD_MOT` and `PPVD_TOP` (Reolink's wrapper, the
  names suggest person/pet/vehicle detection). `BC_PPVD_MOT` suggests a tracking stage.
- Novatek's `vendor_ai` runtime (`vendor_ai_init`, the net parameter calls, CPU post-processing) is statically
  linked into `device`. There is no shared library to load.

## Models

All three are in the rootfs, under `/mnt/src/CNNLib/para/`, in Novatek's network format (an `OINF` header
followed by layer names such as `Conv_670_Y`):

| File | Size | Use |
|---|---|---|
| `ppvd.bin` | 2.7 MB | The detector (ONNX-style graph, input `input.1`). `ppvd_corridor.bin` is referenced but not in this image. |
| `cds.bin` | 1.0 MB | AI day/night (colour or IR) decision, not pet detection. |
| `nvt_bcd_model.bin` | 0.5 MB | Referenced in the `device` strings as a "bcd" model; purpose not confirmed. |

`reid.bin` is also referenced and not present.

## Kernel side

`S10_SysInit2` loads `kdrv_md.ko`, then `kdrv_ai.ko ai_net_supported_num=3` and `kflow_ai.ko`. The count of 3
matches the three models above. `kdrv_ai` and `kflow_ai` are present in the stock kernel tree that `cam` keeps.
`cam` does not load them today.

## What reusing it would take

1. Load `kdrv_ai` and `kflow_ai` from `cam`'s boot path.
2. Reimplement the userspace half of `vendor_ai`: open a network, load the `.bin`, allocate work buffers,
   feed a frame from the HDAL video stream, run, and read the detection output. That code is in `device`,
   undocumented and not in any library we can link.
3. Reimplement the detector's post-processing (anchors, thresholds, non-maximum suppression) and the class
   mapping. The model's output layout is not known yet.
4. Feed the boxes into the existing field-of-view relative move.

Step 2 is the large one. A wrong call can reset the camera, as with the media calls.

## Open questions

- Is the `vendor_ai` ioctl interface to `kdrv_ai` small enough to drive directly?
- What are `ppvd.bin`'s output tensors, and which class index is cat or dog?
- What input size and format does it take, and from which stream does `device` take frames?
- Redistribution: the models are Reolink's and Novatek's. They would have to stay on the camera (copied from the
  stock image at install) and out of this repository.
