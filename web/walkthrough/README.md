# Walkthrough

A small web page that loads an STL and lets you walk through it at human scale, with a keyboard and mouse or in a WebXR headset such as a Meta Quest 3. It is three.js with no build step: three files and a model.

It is separate from the Ferrender app and shares no code with it. Export an STL from Ferrender and view it here.

## Run it

```sh
cd web/walkthrough
python3 -m http.server 8226
```

Open <http://localhost:8226/>. The page fetches three.js 0.185.1 and three-mesh-bvh 0.9.16 from jsDelivr, so the browser needs internet access.

| Input | Does |
|---|---|
| Click | Look around with the mouse. Esc releases it. |
| W A S D or arrows | Walk. Shift goes faster. |
| C | Walk through walls on or off. |
| R | Back to the start. |
| Drop an STL on the page | View that file instead. |

## In a headset

WebXR only runs on a secure page, so `http://<your-mac>:8226` will not offer VR. Two ways round it:

- **USB:** with the headset in developer mode and plugged in, run `adb reverse tcp:8226 tcp:8226` and open `http://localhost:8226/` in the Quest browser. `localhost` counts as secure.
- **HTTPS:** serve the folder through something that gives it a certificate, for example `tailscale serve 8226`, and open that address.

Press **Enter VR**. The left stick walks in the direction you are looking, the right stick turns in 30° steps, and holding either trigger aims a beam: release it over a floor to jump to the ring.

## Use another model

Replace `models/226.stl`, or name a file in the address:

```
http://localhost:8226/?model=models/house.stl
```

| Parameter | Meaning | Default |
|---|---|---|
| `model` | STL to load, relative to this folder | `models/226.stl` |
| `units` | What one STL unit is: `mm`, `cm`, `m`, `in` or `ft` | `mm` |
| `up` | The model's up axis, `z` or `y` | `z` |
| `start` | Where to stand, `X,Y` in model units on the model's ground plane | found automatically |
| `floor` | Height of the floor to start on, in model units | 0 if the model spans it, else its lowest point |
| `eye` | Eye height on a desktop, in metres | 1.65 |

Ferrender exports STL in millimetres with Z up, which are the defaults.

The start is chosen automatically: a spot on the floor, clear of walls, with walls most of the way round it, as near the middle as it can find. If that lands in the wrong room, pass `start`.

## Limits

- One colour for everything. STL carries no materials, so there are no textures, glass or doors that open.
- A doorway must be an actual gap in the mesh. A closed door is a wall.
- Outlines are drawn for models up to 300,000 triangles. Above that only shading separates surfaces.
- No touch controls for phones.
- The headset controls were written against the WebXR standard and have not been run on a headset yet.
