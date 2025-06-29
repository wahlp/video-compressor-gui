GUI to compress videos using FFMPEG so I don't have to pay for Discord Nitro

## Features
- Multiple videos can be queued to be compressed sequentially
- Settings to customise compression level
- Toggleable dark mode

## Requirements
- ffmpeg and ffprobe callable on $PATH

## Tips
- Bitrate and duration are inversely proportional (size = bitrate * duration). Can't have a large amount of both.
- Using lower resolution videos as the input will make the compression process faster
- GPU encoder is much faster, but produces a larger output file size that may exceed calculations and overshoot the size limit
- Lowering the resolution during compression does not make the output look better