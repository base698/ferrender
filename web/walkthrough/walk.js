// Walk through an STL at human scale, on a desktop or in a WebXR headset.
//
// World space is metres with Y up. The model is converted once on load, so everything after
// that (collisions, the floor, teleporting) works in world space against one static mesh.
//
// URL parameters:
//   model=PATH       STL to load (default models/226.stl)
//   units=mm|cm|m|in|ft   what one STL unit is (default mm, which is what Ferrender exports)
//   up=z|y           the model's up axis (default z)
//   start=X,Y        where to stand, in model units on the model's ground plane
//   floor=Z          height of the floor to start on, in model units
//   eye=METRES       eye height on a desktop (default 1.65)

import * as THREE from 'three'
import { STLLoader } from 'three/addons/loaders/STLLoader.js'
import { VRButton } from 'three/addons/webxr/VRButton.js'
import { computeBoundsTree, disposeBoundsTree, acceleratedRaycast } from 'three-mesh-bvh'

THREE.BufferGeometry.prototype.computeBoundsTree = computeBoundsTree
THREE.BufferGeometry.prototype.disposeBoundsTree = disposeBoundsTree
THREE.Mesh.prototype.raycast = acceleratedRaycast

const UNITS = { mm: 0.001, cm: 0.01, m: 1, in: 0.0254, ft: 0.3048 }
const WALK = 1.4          // m/s
const RUN = 3.2
const RADIUS = 0.25       // how close the body gets to a wall
const STEP = 0.4          // tallest ledge that is stepped onto
const DROP = 3.0          // furthest fall that is followed
const SNAP = Math.PI / 6  // headset turn per flick
const REACH = 12          // furthest teleport
const EDGE_LIMIT = 300_000  // triangles above which outlines are skipped

const params = new URLSearchParams(location.search)
const unit = UNITS[params.get('units') ?? 'mm'] ?? UNITS.mm
const zUp = (params.get('up') ?? 'z') !== 'y'
const eye = Number(params.get('eye')) || 1.65

const statusEl = document.getElementById('status')
const say = (text, error = false) => {
  statusEl.textContent = text
  statusEl.classList.toggle('error', error)
}

const renderer = new THREE.WebGLRenderer({ antialias: true })
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
renderer.setSize(window.innerWidth, window.innerHeight)
renderer.xr.enabled = true
renderer.xr.setReferenceSpaceType('local-floor')
document.body.appendChild(renderer.domElement)
document.body.appendChild(VRButton.createButton(renderer))

const scene = new THREE.Scene()
scene.background = new THREE.Color(0xb9c4cf)
scene.add(new THREE.HemisphereLight(0xffffff, 0x6b665e, 1.6))
const sun = new THREE.DirectionalLight(0xffffff, 1.2)
sun.position.set(3, 8, 5)
scene.add(sun)

// The rig stands on the floor. On a desktop the camera sits at eye height inside it; in a
// headset the camera is wherever the head is, measured from the real floor.
const rig = new THREE.Group()
scene.add(rig)
const camera = new THREE.PerspectiveCamera(75, window.innerWidth / window.innerHeight, 0.05, 500)
camera.position.y = eye
camera.rotation.order = 'YXZ'
rig.add(camera)
// A lamp at the head keeps rooms under a ceiling readable and shades walls by distance.
const lamp = new THREE.PointLight(0xfff4e0, 1.5, 14, 1.2)
camera.add(lamp)

const ground = new THREE.Mesh(
  new THREE.PlaneGeometry(400, 400).rotateX(-Math.PI / 2),
  new THREE.MeshStandardMaterial({ color: 0x8f9a86, roughness: 1 }),
)
scene.add(ground)

const ring = new THREE.Mesh(
  new THREE.RingGeometry(0.16, 0.22, 32).rotateX(-Math.PI / 2),
  new THREE.MeshBasicMaterial({ color: 0x2f7bff }),
)
ring.visible = false
scene.add(ring)

const state = {
  model: null,       // the mesh walked through
  edges: null,
  home: null,        // { position, yaw } to return to
  collide: true,
  yaw: 0,
  pitch: 0,
  keys: new Set(),
  aiming: null,      // the controller whose trigger is held
  target: null,      // where a teleport would land
  turned: false,     // the turn stick has not returned to centre yet
}

const ray = new THREE.Raycaster()
ray.firstHitOnly = true
const DOWN = new THREE.Vector3(0, -1, 0)

// ------------------------------------------------------------------ the model

function show(geometry, name) {
  if (state.model) {
    scene.remove(state.model)
    state.model.geometry.disposeBoundsTree()
    state.model.geometry.dispose()
  }
  if (state.edges) {
    scene.remove(state.edges)
    state.edges.geometry.dispose()
  }
  if (zUp) geometry.rotateX(-Math.PI / 2)
  geometry.scale(unit, unit, unit)
  geometry.computeVertexNormals()
  geometry.computeBoundingBox()
  const triangles = geometry.attributes.position.count / 3

  state.edges = null
  if (triangles <= EDGE_LIMIT) {
    state.edges = new THREE.LineSegments(
      new THREE.EdgesGeometry(geometry, 25),
      new THREE.LineBasicMaterial({ color: 0x3a3f45 }),
    )
    scene.add(state.edges)
  }
  geometry.computeBoundsTree()
  state.model = new THREE.Mesh(geometry, new THREE.MeshStandardMaterial({
    color: 0xe2ddd4, roughness: 0.9, flatShading: true, side: THREE.DoubleSide,
  }))
  scene.add(state.model)

  const box = geometry.boundingBox
  const size = box.getSize(new THREE.Vector3())
  ground.position.set((box.min.x + box.max.x) / 2, box.min.y - 0.02, (box.min.z + box.max.z) / 2)

  state.home = findStart(box)
  goHome()
  say(`${name}: ${triangles.toLocaleString()} triangles, ` +
    `${size.x.toFixed(1)} × ${size.z.toFixed(1)} m, ${size.y.toFixed(1)} m tall.`)
}

async function load(url) {
  say('Loading…')
  try {
    const geometry = await new STLLoader().loadAsync(url)
    show(geometry, url.split('/').pop())
  } catch (e) {
    say(`Could not load ${url}: ${e.message ?? e}`, true)
  }
}

// ------------------------------------------------------------------ where things are

function hit(origin, direction, far) {
  if (!state.model) return null
  ray.set(origin, direction)
  ray.near = 0
  ray.far = far
  return ray.intersectObject(state.model, false)[0] ?? null
}

/// Height of the surface under a point, looking from a step above the feet.
function floorUnder(x, z, feet) {
  const h = hit(new THREE.Vector3(x, feet + STEP, z), DOWN, STEP + DROP)
  return h ? h.point.y : null
}

/// The nearest thing in the way of a body at `from` (feet height `feet`) moving `distance`
/// along the horizontal unit vector `dir`.
function inTheWay(from, feet, dir, distance) {
  const side = new THREE.Vector3(-dir.z, 0, dir.x)
  let nearest = null
  for (const height of [STEP + 0.05, 1.0, 1.5]) {
    for (const offset of [0, -0.7 * RADIUS, 0.7 * RADIUS]) {
      const origin = new THREE.Vector3(from.x, feet + height, from.z).addScaledVector(side, offset)
      const h = hit(origin, dir, distance + RADIUS)
      if (h && (!nearest || h.distance < nearest.distance)) nearest = h
    }
  }
  return nearest
}

/// The part of a horizontal move that walls allow: all of it, a slide along the wall, or none.
function allowed(from, feet, move) {
  const length = move.length()
  if (length < 1e-6 || !state.collide) return move
  const dir = move.clone().divideScalar(length)
  const wall = inTheWay(from, feet, dir, length)
  if (!wall) return move
  const normal = wall.face.normal.clone().setY(0)
  if (normal.lengthSq() < 1e-6) return move.set(0, 0, 0)
  normal.normalize()
  const slide = move.clone().addScaledVector(normal, -move.dot(normal))
  const slid = slide.length()
  if (slid < 1e-6) return move.set(0, 0, 0)
  if (inTheWay(from, feet, slide.clone().divideScalar(slid), slid)) return move.set(0, 0, 0)
  return slide
}

/// A place to stand: on a floor, clear of walls, enclosed, and near the middle.
function findStart(box) {
  const centre = box.getCenter(new THREE.Vector3())
  const given = params.get('floor')
  const level = given !== null ? Number(given) * unit
    : box.min.y <= 0 && box.max.y > 0 ? 0 : box.min.y
  const chosen = (params.get('start') ?? '').split(',').map(Number)
  if (chosen.length === 2 && chosen.every(Number.isFinite)) {
    const x = chosen[0] * unit
    const z = zUp ? -chosen[1] * unit : chosen[1] * unit
    return { position: new THREE.Vector3(x, floorUnder(x, z, level + 1) ?? level, z), yaw: 0 }
  }

  const spacing = Math.max(0.25, Math.sqrt((box.max.x - box.min.x) * (box.max.z - box.min.z) / 2500))
  const around = Array.from({ length: 8 }, (_, i) =>
    new THREE.Vector3(Math.sin(i * Math.PI / 4), 0, Math.cos(i * Math.PI / 4)))
  let best = null
  for (let x = box.min.x + spacing / 2; x < box.max.x; x += spacing) {
    for (let z = box.min.z + spacing / 2; z < box.max.z; z += spacing) {
      const y = floorUnder(x, z, level + 0.6)
      if (y === null || Math.abs(y - level) > 0.6) continue
      const chest = new THREE.Vector3(x, y + 1.0, z)
      let clear = Infinity, walls = 0, longest = 0, facing = 0
      around.forEach((dir, i) => {
        const h = hit(chest, dir, 60)
        if (h) walls += 1
        const d = h ? h.distance : 60
        clear = Math.min(clear, d)
        if (h && d > longest) { longest = d; facing = i }
      })
      if (clear < 2 * RADIUS || walls < 7) continue
      const score = Math.min(clear, 1.5) - 0.05 * Math.hypot(x - centre.x, z - centre.z)
      if (!best || score > best.score) {
        // Yaw 0 looks down -Z, and `around[i]` points along (sin, cos).
        best = { score, position: new THREE.Vector3(x, y, z), yaw: facing * Math.PI / 4 + Math.PI }
      }
    }
  }
  if (best) return best
  // Nothing enclosed: stand outside and look at the model.
  const back = Math.max(box.max.x - box.min.x, box.max.z - box.min.z) * 0.8 + 2
  return { position: new THREE.Vector3(centre.x, level, box.max.z + back), yaw: 0 }
}

function goHome() {
  if (!state.home) return
  state.yaw = state.home.yaw
  state.pitch = 0
  rig.rotation.y = state.yaw
  rig.position.copy(state.home.position)
  if (renderer.xr.isPresenting) putHeadAt(state.home.position)
}

// ------------------------------------------------------------------ moving

const head = new THREE.Vector3()
const forward = new THREE.Vector3()

/// Move the rig so the head is above `point` and the feet are on it.
function putHeadAt(point) {
  camera.getWorldPosition(head)
  rig.position.x += point.x - head.x
  rig.position.z += point.z - head.z
  rig.position.y = point.y
}

/// Turn the rig about the head, so a headset turn does not swing the wearer sideways.
function turn(angle) {
  camera.getWorldPosition(head)
  const before = head.clone()
  rig.rotation.y += angle
  rig.updateMatrixWorld(true)
  camera.getWorldPosition(head)
  rig.position.x += before.x - head.x
  rig.position.z += before.z - head.z
  state.yaw = rig.rotation.y
}

/// Walk `ahead` and `right` metres relative to where the head faces.
function walk(ahead, right, dt) {
  if (!ahead && !right) return settle(dt)
  camera.getWorldDirection(forward)
  forward.y = 0
  if (forward.lengthSq() < 1e-6) return
  forward.normalize()
  const move = new THREE.Vector3()
    .addScaledVector(forward, ahead)
    .addScaledVector(new THREE.Vector3(-forward.z, 0, forward.x), right)
  camera.getWorldPosition(head)
  const step = allowed(head, rig.position.y, move)
  rig.position.x += step.x
  rig.position.z += step.z
  settle(dt)
}

/// Follow the floor under the head: up a step at once, down at falling speed.
function settle(dt) {
  camera.getWorldPosition(head)
  const y = floorUnder(head.x, head.z, rig.position.y)
  if (y === null) return
  rig.position.y = y >= rig.position.y ? y : Math.max(y, rig.position.y - 4 * dt)
}

function keyboard(dt) {
  const k = state.keys
  const ahead = (k.has('KeyW') || k.has('ArrowUp') ? 1 : 0) - (k.has('KeyS') || k.has('ArrowDown') ? 1 : 0)
  const right = (k.has('KeyD') || k.has('ArrowRight') ? 1 : 0) - (k.has('KeyA') || k.has('ArrowLeft') ? 1 : 0)
  const speed = (k.has('ShiftLeft') || k.has('ShiftRight') ? RUN : WALK) * dt
  const scale = ahead && right ? Math.SQRT1_2 : 1
  walk(ahead * speed * scale, right * speed * scale, dt)
}

function sticks(dt) {
  let ahead = 0, right = 0, twist = 0
  for (const source of renderer.xr.getSession()?.inputSources ?? []) {
    const axes = source.gamepad?.axes
    if (!axes || axes.length < 4) continue
    if (source.handedness === 'right') twist = axes[2]
    else { right = axes[2]; ahead = -axes[3] }
  }
  const dead = v => (Math.abs(v) < 0.15 ? 0 : v)
  walk(dead(ahead) * WALK * dt, dead(right) * WALK * dt, dt)
  if (Math.abs(twist) < 0.3) state.turned = false
  else if (Math.abs(twist) > 0.6 && !state.turned) {
    state.turned = true
    turn(-Math.sign(twist) * SNAP)
  }
}

// ------------------------------------------------------------------ teleporting

const controllers = [0, 1].map(i => {
  const controller = renderer.xr.getController(i)
  const beam = new THREE.Line(
    new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(), new THREE.Vector3(0, 0, -1)]),
    new THREE.LineBasicMaterial({ color: 0x2f7bff }),
  )
  beam.visible = false
  controller.add(beam)
  controller.userData.beam = beam
  controller.addEventListener('selectstart', () => { state.aiming = controller })
  controller.addEventListener('selectend', () => {
    if (state.aiming !== controller) return
    if (state.target) putHeadAt(state.target)
    state.aiming = null
  })
  rig.add(controller)
  return controller
})

function aim() {
  state.target = null
  for (const c of controllers) c.userData.beam.visible = c === state.aiming
  const c = state.aiming
  if (c) {
    const from = c.getWorldPosition(new THREE.Vector3())
    const along = new THREE.Vector3(0, 0, -1).transformDirection(c.matrixWorld)
    const h = hit(from, along, REACH)
    c.userData.beam.scale.z = h ? h.distance : REACH
    // Only something you could stand on: facing up, whichever way its triangle is wound.
    if (h && Math.abs(h.face.normal.y) > 0.7 && along.y < 0) state.target = h.point.clone()
  }
  ring.visible = !!state.target
  if (state.target) ring.position.copy(state.target).y += 0.01
}

// ------------------------------------------------------------------ input

const canvas = renderer.domElement
canvas.addEventListener('click', () => {
  if (!renderer.xr.isPresenting) canvas.requestPointerLock?.()
})
document.addEventListener('pointerlockchange', () => {
  document.body.classList.toggle('walking', document.pointerLockElement === canvas)
})
document.addEventListener('mousemove', e => {
  if (document.pointerLockElement !== canvas) return
  state.yaw -= e.movementX * 0.0022
  state.pitch = THREE.MathUtils.clamp(state.pitch - e.movementY * 0.0022, -1.5, 1.5)
})
window.addEventListener('keydown', e => {
  state.keys.add(e.code)
  if (e.code === 'KeyC') {
    state.collide = !state.collide
    say(state.collide ? 'Walls are solid.' : 'Walking through walls.')
  }
  if (e.code === 'KeyR') goHome()
})
window.addEventListener('keyup', e => state.keys.delete(e.code))
window.addEventListener('blur', () => state.keys.clear())
window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight
  camera.updateProjectionMatrix()
  renderer.setSize(window.innerWidth, window.innerHeight)
})

window.addEventListener('dragover', e => { e.preventDefault(); document.body.classList.add('dropping') })
window.addEventListener('dragleave', () => document.body.classList.remove('dropping'))
window.addEventListener('drop', async e => {
  e.preventDefault()
  document.body.classList.remove('dropping')
  const file = e.dataTransfer?.files?.[0]
  if (!file) return
  try {
    show(new STLLoader().parse(await file.arrayBuffer()), file.name)
  } catch (err) {
    say(`Could not read ${file.name}: ${err.message ?? err}`, true)
  }
})

renderer.xr.addEventListener('sessionstart', () => {
  document.exitPointerLock?.()
  // The headset supplies the head's height and direction from here on.
  camera.position.set(0, 0, 0)
  camera.rotation.set(0, 0, 0)
})
renderer.xr.addEventListener('sessionend', () => {
  state.aiming = null
  ring.visible = false
  camera.position.set(0, eye, 0)
  state.pitch = 0
})

// ------------------------------------------------------------------ the loop

let last = performance.now()
renderer.setAnimationLoop(() => {
  const now = performance.now()
  const dt = Math.min((now - last) / 1000, 0.1)
  last = now
  if (renderer.xr.isPresenting) {
    sticks(dt)
    aim()
  } else {
    rig.rotation.y = state.yaw
    camera.rotation.x = state.pitch
    rig.updateMatrixWorld(true)
    keyboard(dt)
  }
  renderer.render(scene, camera)
})

// For tests and for poking at from the console.
window.walkthrough = { state, rig, camera, renderer, load, goHome, walk, turn, putHeadAt }

load(params.get('model') ?? 'models/226.stl')
