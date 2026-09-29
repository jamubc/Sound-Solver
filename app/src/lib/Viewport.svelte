<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import * as THREE from 'three';
  import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
  import { TransformControls } from 'three/examples/jsm/controls/TransformControls.js';
  import { ViewHelper } from 'three/examples/jsm/helpers/ViewHelper.js';
  import { Line2 } from 'three/examples/jsm/lines/Line2.js';
  import { LineGeometry } from 'three/examples/jsm/lines/LineGeometry.js';
  import { LineMaterial } from 'three/examples/jsm/lines/LineMaterial.js';
  import { CSS2DObject, CSS2DRenderer } from 'three/examples/jsm/renderers/CSS2DRenderer.js';
  import { MeshBVH, acceleratedRaycast } from 'three-mesh-bvh';
  import { lengthUnit, mm, shown } from './format';
  import { app, edit, editFabrication, REFERENCE_COLOURS, type Selection } from './state.svelte';
  import type { Layout, PieceLayout } from './types/api';

  type Vec3 = [number, number, number];
  type Pick = Selection;

  // Colours by pipe material family; the selected route in the accent colour.
  const MATERIAL_COLOUR: Record<string, number> = { '409': 0x8d949e, '304': 0xb9c2cc, '321': 0xb9c2cc, 'ti-gr5': 0x9d95c9 };
  const ACCENT = 0x6aa9ff;
  const HOVER = 0x1c2c44;
  /** Standard views: where the camera looks from, in vehicle axes (X forward, Y left, Z up). */
  const VIEWS: [string, Vec3, string][] = [
    ['Iso', [0.25, -0.85, 0.45], 'From the right, front and above'],
    ['Top', [0, -0.001, 1], 'From above'],
    ['Under', [0, -0.001, -1], 'From below: the underbody side'],
    ['Side', [0, -1, 0], 'From the right side'],
    ['Front', [1, 0, 0], 'From the front'],
    ['Rear', [-1, 0, 0], 'From behind'],
  ];

  let host: HTMLDivElement;
  let renderer: THREE.WebGLRenderer;
  let labels: CSS2DRenderer;
  let triad: ViewHelper;
  let scene: THREE.Scene;
  let camera: THREE.PerspectiveCamera;
  let orbit: OrbitControls;
  let gizmo: TransformControls;
  let content = new THREE.Group();
  let handles = new Map<string, THREE.Mesh>();
  let lineMaterials: LineMaterial[] = [];
  let framed = false;
  let scanObject = $state.raw<THREE.Mesh | null>(null);
  /** Materials lit by the pointer hovering over their route or element. */
  let hovered: THREE.MeshStandardMaterial[] = [];

  const v3 = (p: readonly number[]) => new THREE.Vector3(p[0], p[1], p[2]);
  const key = (route: string, index: number) => `${route}#${index}`;

  /** Mesh of a coaxial frustum from `a` (diameter `da`) to `b` (diameter `db`). */
  function frustum(a: THREE.Vector3, b: THREE.Vector3, da: number, db: number, material: THREE.Material) {
    const axis = b.clone().sub(a);
    const mesh = new THREE.Mesh(new THREE.CylinderGeometry(db / 2, da / 2, axis.length(), 32, 1), material);
    mesh.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), axis.clone().normalize());
    mesh.position.copy(a.clone().add(b).multiplyScalar(0.5));
    return mesh;
  }

  /** Arc length along the route of the centreline point nearest `p` on `piece`, mm. */
  function along(piece: PieceLayout, p: THREE.Vector3): number {
    if (piece.kind === 'straight') {
      const a = v3(piece.from);
      const d = v3(piece.to).sub(a);
      const len = d.length();
      return piece.s0_mm + Math.min(len, Math.max(0, p.clone().sub(a).dot(d.normalize())));
    }
    const c = v3(piece.centre);
    const u = v3(piece.from).sub(c).normalize();
    const w = v3(piece.to).sub(c);
    const v = w.clone().sub(u.clone().multiplyScalar(u.dot(w))).normalize();
    const q = p.clone().sub(c);
    const angle = Math.min(Math.max(Math.atan2(q.dot(v), q.dot(u)), 0), (piece.angle_deg * Math.PI) / 180);
    return piece.s0_mm + piece.radius_mm * angle;
  }

  /** Points along a centreline piece (arcs every ~5°). */
  function samples(piece: PieceLayout): THREE.Vector3[] {
    if (piece.kind === 'straight') return [v3(piece.from), v3(piece.to)];
    const c = v3(piece.centre);
    const u = v3(piece.from).sub(c).normalize();
    const w = v3(piece.to).sub(c);
    const v = w.clone().sub(u.clone().multiplyScalar(u.dot(w))).normalize();
    const theta = (piece.angle_deg * Math.PI) / 180;
    const n = Math.max(2, Math.ceil(piece.angle_deg / 5));
    return Array.from({ length: n + 1 }, (_, i) => {
      const t = (theta * i) / n;
      return c.clone().add(u.clone().multiplyScalar(piece.radius_mm * Math.cos(t))).add(v.clone().multiplyScalar(piece.radius_mm * Math.sin(t)));
    });
  }

  function dispose(group: THREE.Object3D) {
    group.traverse((o) => {
      if (o instanceof THREE.Mesh || o instanceof Line2 || o instanceof THREE.Line) {
        o.geometry.dispose();
        (Array.isArray(o.material) ? o.material : [o.material]).forEach((m) => m.dispose());
      }
    });
  }

  function clear() {
    gizmo?.detach();
    dispose(content);
    content.traverse((o) => {
      if (o instanceof CSS2DObject) o.element.remove();
    });
    scene.remove(content);
    content = new THREE.Group();
    handles = new Map();
    lineMaterials = [];
    hovered = [];
  }

  /** A text label pinned to a point of the scene. */
  function label(text: string, at: THREE.Vector3, kind: 'dim' | 'tag') {
    const element = document.createElement('div');
    element.className = kind;
    element.textContent = text;
    const object = new CSS2DObject(element);
    object.position.copy(at);
    content.add(object);
  }

  function build(layout: Layout, selection: Selection | null) {
    clear();
    const selectedRoute = selection?.kind === 'route' ? selection.id : selection?.kind === 'vertex' ? selection.route : null;
    for (const r of layout.routes) {
      const colour = r.id === selectedRoute ? ACCENT : (MATERIAL_COLOUR[r.material] ?? 0x8d949e);
      const material = new THREE.MeshStandardMaterial({ color: colour, metalness: 0.55, roughness: 0.45 });
      const pick: Pick = { kind: 'route', id: r.id };
      const centreline: THREE.Vector3[] = [];
      for (const piece of r.pieces) {
        const pts = samples(piece);
        centreline.push(...(centreline.length ? pts.slice(1) : pts));
        const mesh =
          piece.kind === 'straight'
            ? frustum(pts[0], pts[1], r.od_mm, r.od_mm, material)
            : new THREE.Mesh(new THREE.TubeGeometry(new THREE.CatmullRomCurve3(pts), pts.length * 2, r.od_mm / 2, 24, false), material);
        mesh.userData.pick = pick;
        mesh.userData.piece = piece;
        content.add(mesh);
        // Dimensions of the selected pipe: every straight's length, every bend's angle and radius.
        if (r.id !== selectedRoute) continue;
        const middle = pts[Math.floor(pts.length / 2)].clone();
        if (piece.kind === 'straight') {
          const length = pts[0].distanceTo(pts[1]);
          if (length >= 20) label(mm(length, 0), pts[0].clone().lerp(pts[1], 0.5), 'dim');
        } else {
          label(`${piece.angle_deg.toFixed(0)}° R${shown(piece.radius_mm)} ${lengthUnit()}`, middle, 'dim');
        }
      }
      // The flow solver's x-axis, drawn over the pipe.
      const geometry = new LineGeometry();
      geometry.setPositions(centreline.flatMap((p) => [p.x, p.y, p.z]));
      const lm = new LineMaterial({ color: 0xffd479, linewidth: 1.5, depthTest: false, transparent: true, opacity: 0.8 });
      lineMaterials.push(lm);
      const line = new Line2(geometry, lm);
      line.renderOrder = 2;
      content.add(line);
      r.points.slice(1, -1).forEach((p, i) => {
        const chosen = selection?.kind === 'vertex' && selection.route === r.id && selection.index === i;
        const handle = new THREE.Mesh(
          new THREE.SphereGeometry(Math.max(0.45 * r.od_mm, 10), 20, 14),
          new THREE.MeshStandardMaterial({ color: chosen ? 0xffffff : 0xf5a623, emissive: chosen ? 0x553300 : 0x000000 }),
        );
        handle.position.copy(v3(p));
        handle.userData.pick = { kind: 'vertex', route: r.id, index: i } satisfies Pick;
        handle.renderOrder = 3;
        handles.set(key(r.id, i), handle);
        content.add(handle);
      });
      // Hangers: a rod up from the pipe towards the body.
      const hanger = new THREE.MeshStandardMaterial({ color: 0x2b2f36, metalness: 0.6, roughness: 0.4 });
      for (const h of r.hangers) {
        const rod = frustum(v3(h), v3(h).add(new THREE.Vector3(0, 0, r.od_mm / 2 + 60)), 8, 8, hanger);
        rod.userData.pick = pick;
        content.add(rod);
      }
    }
    for (const e of layout.elements) {
      const chosen = selection?.kind === 'element' && selection.id === e.id;
      const material = new THREE.MeshStandardMaterial({
        color: chosen ? ACCENT : 0x4f6b8a,
        metalness: 0.3,
        roughness: 0.6,
        transparent: true,
        opacity: 0.6,
      });
      const pick: Pick = { kind: 'element', id: e.id };
      for (const f of e.body) {
        const mesh = frustum(v3(f.from), v3(f.to), f.d_from_mm, f.d_to_mm, material);
        mesh.userData.pick = pick;
        content.add(mesh);
      }
      const body = e.body[Math.floor(e.body.length / 2)];
      label(e.id, body ? v3(body.from).lerp(v3(body.to), 0.5) : v3(e.ports[0]?.position ?? [0, 0, 0]), 'tag');
      for (const p of e.ports) {
        const port = new THREE.Mesh(new THREE.SphereGeometry(7, 12, 8), new THREE.MeshBasicMaterial({ color: 0x5fd08a }));
        port.position.copy(v3(p.position));
        port.userData.pick = pick;
        content.add(port);
      }
    }
    scene.add(content);
    if (selection?.kind === 'vertex') {
      const handle = handles.get(key(selection.route, selection.index));
      if (handle) gizmo.attach(handle);
    }
    resize();
    if (!framed) frame();
  }

  /** Everything drawn: the system and the scan. */
  function everything() {
    const box = new THREE.Box3().setFromObject(content);
    if (scanObject) box.union(new THREE.Box3().setFromObject(scanObject));
    return box;
  }

  /** Fits `box` in view, looking from `from` (default: the current direction). */
  function fit(box: THREE.Box3, from?: readonly number[]) {
    if (box.isEmpty()) return;
    const direction = from ? v3(from).normalize() : camera.position.clone().sub(orbit.target).normalize();
    const sphere = box.getBoundingSphere(new THREE.Sphere());
    const fov = (camera.fov * Math.PI) / 180;
    const narrowest = Math.min(fov, 2 * Math.atan(Math.tan(fov / 2) * camera.aspect));
    const distance = (1.05 * Math.max(sphere.radius, 50)) / Math.sin(narrowest / 2);
    orbit.target.copy(sphere.center);
    camera.position.copy(sphere.center).addScaledVector(direction, distance);
    camera.near = distance / 200;
    camera.far = distance * 50;
    camera.updateProjectionMatrix();
    orbit.update();
  }

  /** Fits the whole system (and the scan) in view, seen from the car's right side, slightly from above. */
  function frame() {
    fit(everything(), VIEWS[0][1]);
    framed = true;
  }

  const fitAll = () => fit(everything());

  /** Fits the selected route, bend point or element in view. */
  function fitSelection() {
    const s = app.selection;
    if (!s) return;
    const box = new THREE.Box3();
    content.traverse((o) => {
      const pick = o.userData.pick as Pick | undefined;
      const hit =
        pick &&
        (s.kind === 'element'
          ? pick.kind === 'element' && pick.id === s.id
          : s.kind === 'route'
            ? pick.kind === 'route' && pick.id === s.id
            : pick.kind === 'vertex' && pick.route === s.route && pick.index === s.index);
      if (hit) box.expandByObject(o);
    });
    fit(box);
  }

  function hover(pick: Pick | undefined) {
    hovered.forEach((m) => m.emissive.setHex(0x000000));
    hovered = [];
    if (!pick || pick.kind === 'vertex') return;
    content.traverse((o) => {
      const p = o.userData.pick as Pick | undefined;
      const same = p && p.kind !== 'vertex' && p.kind === pick.kind && p.id === pick.id;
      if (same && o instanceof THREE.Mesh && o.material instanceof THREE.MeshStandardMaterial) {
        o.material.emissive.setHex(HOVER);
        hovered.push(o.material);
      }
    });
  }

  function resize() {
    if (!host || !renderer) return;
    const { clientWidth: w, clientHeight: h } = host;
    renderer.setSize(w, h, false);
    labels.setSize(w, h);
    camera.aspect = w / Math.max(h, 1);
    camera.updateProjectionMatrix();
    lineMaterials.forEach((m) => m.resolution.set(w, h));
  }

  // F fits everything; Esc drops the selection and any scan pick in progress.
  function keys(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.target instanceof HTMLElement && e.target.closest('input, select, textarea')) return;
    if (e.key === 'f' || e.key === 'F') fitAll();
    else if (e.key === 'Escape') {
      app.selection = null;
      app.picking = null;
    }
  }

  /** A via point dragged to a new place becomes an edit of its route. */
  function commitDrag() {
    const object = gizmo.object;
    const pick = object?.userData.pick as Pick | undefined;
    if (!object || pick?.kind !== 'vertex') return;
    const p = object.position;
    const at: Vec3 = [Math.round(p.x), Math.round(p.y), Math.round(p.z)];
    edit((project) => {
      const r = project.system.routes.find((x) => x.id === pick.route);
      if (r?.via_mm) r.via_mm[pick.index] = at;
    });
  }

  onMount(() => {
    renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.autoClear = false;
    host.appendChild(renderer.domElement);
    labels = new CSS2DRenderer();
    labels.domElement.className = 'labels';
    host.appendChild(labels.domElement);
    scene = new THREE.Scene();
    scene.background = new THREE.Color(0x11151d);
    camera = new THREE.PerspectiveCamera(35, 1, 5, 200000);
    camera.up.set(0, 0, 1);
    camera.position.set(2500, 3000, 1800);
    orbit = new OrbitControls(camera, renderer.domElement);
    orbit.enableDamping = true;
    orbit.zoomToCursor = true;
    // The corner triad shows which way the vehicle axes point.
    triad = new ViewHelper(camera, renderer.domElement);
    triad.setLabels('X', 'Y', 'Z');
    scene.add(new THREE.HemisphereLight(0xdde6ff, 0x1a1f28, 1.4));
    const sun = new THREE.DirectionalLight(0xffffff, 1.8);
    sun.position.set(1500, 2000, 4000);
    scene.add(sun);
    const axes = new THREE.AxesHelper(250);
    scene.add(axes);
    gizmo = new TransformControls(camera, renderer.domElement);
    gizmo.setSize(0.8);
    gizmo.addEventListener('dragging-changed', (e) => {
      orbit.enabled = !e.value;
      if (!e.value) commitDrag();
    });
    scene.add(gizmo.getHelper());

    let down: { x: number; y: number } | null = null;
    const raycaster = new THREE.Raycaster();
    renderer.domElement.addEventListener('pointerdown', (e) => (down = { x: e.clientX, y: e.clientY }));
    raycaster.firstHitOnly = true;
    renderer.domElement.addEventListener('pointerup', (e) => {
      if (!down || gizmo.dragging || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) return;
      const rect = renderer.domElement.getBoundingClientRect();
      const ndc = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
      raycaster.setFromCamera(ndc, camera);
      const k = app.picking;
      if (k !== null && scanObject) {
        const [onScan] = raycaster.intersectObject(scanObject, false);
        if (!onScan) return;
        // Back into scan coordinates, which the project stores.
        const p = scanObject.worldToLocal(onScan.point.clone());
        editFabrication((project) => {
          const scan = project.fabrication?.scan;
          if (scan) scan.scan_points[k] = [p.x, p.y, p.z];
        });
        app.picking = null;
        return;
      }
      const hit = raycaster.intersectObjects(content.children, false).find((h) => h.object.userData.pick);
      const pick = hit?.object.userData.pick as Pick | undefined;
      const piece = hit?.object.userData.piece as PieceLayout | undefined;
      app.selection = pick?.kind === 'route' && piece ? { ...pick, s_mm: along(piece, hit!.point) } : (pick ?? null);
    });
    // Hover: light what a click would select, at most once a frame.
    let pointer: THREE.Vector2 | null = null;
    renderer.domElement.addEventListener('pointermove', (e) => {
      const rect = renderer.domElement.getBoundingClientRect();
      pointer = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    });
    renderer.domElement.addEventListener('pointerleave', () => {
      pointer = null;
      hover(undefined);
    });

    const observer = new ResizeObserver(resize);
    observer.observe(host);
    renderer.setAnimationLoop(() => {
      orbit.update();
      if (pointer && !gizmo.dragging && app.picking === null) {
        raycaster.setFromCamera(pointer, camera);
        const pick = raycaster.intersectObjects(content.children, false).find((h) => h.object.userData.pick)?.object
          .userData.pick as Pick | undefined;
        hover(pick);
        renderer.domElement.style.cursor = pick ? 'pointer' : '';
        pointer = null;
      }
      renderer.clear();
      renderer.render(scene, camera);
      triad.render(renderer);
      labels.render(scene, camera);
    });
    window.addEventListener('keydown', keys);
    return () => {
      window.removeEventListener('keydown', keys);
      observer.disconnect();
      renderer.setAnimationLoop(null);
      clear();
      triad.dispose();
      gizmo.dispose();
      orbit.dispose();
      renderer.dispose();
    };
  });

  $effect(() => {
    const layout = app.layout;
    const selection = app.selection;
    if (renderer && layout) build(layout, selection);
  });

  // A new project is framed afresh.
  $effect(() => {
    void app.path;
    void app.project?.name;
    framed = false;
  });

  /** Scan coordinates to the car's: the placement the reference points give, else only the unit. */
  const scanMatrix = $derived.by(() => {
    const unit = app.project?.fabrication?.scan?.unit_mm ?? 1;
    const scaling = new THREE.Matrix4().makeScale(unit, unit, unit);
    const p = app.clearance?.placement;
    if (!p) return scaling;
    const [r, t] = [p.rotation, p.translation];
    return new THREE.Matrix4()
      .set(r[0][0], r[0][1], r[0][2], t[0], r[1][0], r[1][1], r[1][2], t[1], r[2][0], r[2][1], r[2][2], t[2], 0, 0, 0, 1)
      .multiply(scaling);
  });

  // The scan, see-through so the pipes show beneath it; picked through a BVH.
  $effect(() => {
    const mesh = app.scanMesh;
    if (!scene || !mesh) return;
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute('position', new THREE.BufferAttribute(mesh.positions, 3));
    geometry.setIndex(new THREE.BufferAttribute(mesh.index, 1));
    geometry.boundsTree = new MeshBVH(geometry);
    const material = new THREE.MeshStandardMaterial({
      color: 0x9aa6b8,
      flatShading: true,
      side: THREE.DoubleSide,
      transparent: true,
      opacity: 0.35,
      depthWrite: false,
    });
    const object = new THREE.Mesh(geometry, material);
    object.raycast = acceleratedRaycast;
    object.matrixAutoUpdate = false;
    object.matrix.copy(untrack(() => scanMatrix));
    scene.add(object);
    scanObject = object;
    untrack(frame);
    return () => {
      scene.remove(object);
      geometry.dispose();
      material.dispose();
      scanObject = null;
    };
  });

  $effect(() => {
    if (!scanObject) return;
    scanObject.matrix.copy(scanMatrix);
    scanObject.matrixWorldNeedsUpdate = true;
  });

  // Reference points (solid on the scan, wireframe where they belong on the car) and contacts.
  $effect(() => {
    const scan = app.project?.fabrication?.scan;
    const contacts = app.clearance?.contacts ?? [];
    if (!scene) return;
    const group = new THREE.Group();
    const marker = (at: THREE.Vector3, radius: number, color: string | number, wireframe = false) => {
      const m = new THREE.Mesh(new THREE.SphereGeometry(radius, 16, 12), new THREE.MeshBasicMaterial({ color, wireframe }));
      m.position.copy(at);
      group.add(m);
    };
    const given = (p: readonly number[]) => p.some((x) => x !== 0);
    scan?.scan_points.forEach((p, k) => {
      if (given(p)) marker(v3(p).applyMatrix4(scanMatrix), 35, REFERENCE_COLOURS[k]);
      if (given(scan.vehicle_points_mm[k])) marker(v3(scan.vehicle_points_mm[k]), 50, REFERENCE_COLOURS[k], true);
    });
    for (const c of contacts) {
      marker(v3(c.scan_point), 12, 0xff6b6b);
      const line = new THREE.Line(
        new THREE.BufferGeometry().setFromPoints([v3(c.at), v3(c.scan_point)]),
        new THREE.LineBasicMaterial({ color: 0xff6b6b }),
      );
      group.add(line);
    }
    scene.add(group);
    return () => {
      scene.remove(group);
      dispose(group);
    };
  });

  $effect(() => {
    if (renderer) renderer.domElement.style.cursor = app.picking !== null && app.scanMesh ? 'crosshair' : '';
  });

  $effect(() => {
    const ground = app.project?.ambient.ground_z_mm;
    if (!scene || ground === undefined) return;
    const grid = new THREE.GridHelper(8000, 80, 0x2f3949, 0x222a36);
    grid.rotation.x = Math.PI / 2;
    grid.position.z = ground;
    scene.add(grid);
    return () => {
      scene.remove(grid);
      grid.geometry.dispose();
      (grid.material as THREE.Material).dispose();
    };
  });
</script>

<div class="viewport" bind:this={host} data-testid="viewport">
  <div class="legend muted">
    X forward · Y left · Z up · origin at the downpipe flange<br />
    {#if app.picking !== null}
      Click the scan to place reference point {app.picking + 1} · Esc to stop
    {:else}
      Click to select · drag a bend point's arrows to move it · drag to orbit, right-drag to pan, scroll to zoom
      at the cursor · F fits · Esc clears
    {/if}
  </div>
  <div class="views" role="toolbar" aria-label="View">
    <button onclick={fitAll} title="Fit everything in view (F)">Fit</button>
    <button onclick={fitSelection} disabled={!app.selection} title="Fit the selection in view">Selection</button>
    <span class="sep"></span>
    {#each VIEWS as [name, from, title] (name)}
      <button onclick={() => fit(everything(), from)} {title}>{name}</button>
    {/each}
  </div>
</div>

<style>
  .viewport {
    position: absolute;
    inset: 0;
  }

  .viewport :global(canvas) {
    display: block;
    width: 100%;
    height: 100%;
  }

  .legend {
    position: absolute;
    left: 10px;
    bottom: 8px;
    max-width: calc(100% - 160px);
    font-size: 11px;
    pointer-events: none;
  }

  .views {
    position: absolute;
    right: 8px;
    top: 8px;
    display: flex;
    gap: 2px;
    padding: 3px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: rgba(24, 29, 39, 0.85);
  }

  .views button {
    padding: 2px 7px;
    font-size: 11.5px;
    border-color: transparent;
    background: none;
  }

  .views button:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .sep {
    width: 1px;
    margin: 2px 3px;
    background: var(--line);
  }

  .viewport :global(.labels) {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  .viewport :global(.dim),
  .viewport :global(.tag) {
    font: 11px/1.2 ui-monospace, 'SF Mono', Menlo, monospace;
    padding: 1px 5px;
    border-radius: 3px;
    white-space: nowrap;
  }

  .viewport :global(.dim) {
    color: #ffd479;
    background: rgba(17, 21, 29, 0.88);
    border: 1px solid #5a4a22;
  }

  .viewport :global(.tag) {
    color: var(--text);
    background: rgba(31, 37, 49, 0.8);
    border: 1px solid var(--line);
  }
</style>
