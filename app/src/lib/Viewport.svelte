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
  import { lengthUnit, mm, rpm as rpmText, shown } from './format';
  import Icon from './Icon.svelte';
  import { play } from './player.svelte';
  import { app, cancelTimeDomain, edit, editFabrication, REFERENCE_COLOURS, type Selection } from './state.svelte';
  import type { Layout, PieceLayout } from './types/api';
  import { persist, show, ui, view, VIEWS } from './ui.svelte';

  type Vec3 = [number, number, number];
  type Pick = Selection;

  // Colours by pipe material family; the selected pipe in the accent colour.
  const MATERIAL_COLOUR: Record<string, number> = { '409': 0x8d949e, '304': 0xb9c2cc, '321': 0xb9c2cc, 'ti-gr5': 0x9d95c9 };
  const ACCENT = 0x4c9dff;
  const HOVER = 0x1c2c44;

  let host: HTMLDivElement;
  let renderer: THREE.WebGLRenderer;
  let labels: CSS2DRenderer;
  let triad: ViewHelper;
  let scene: THREE.Scene;
  let perspective: THREE.PerspectiveCamera;
  let orthographic: THREE.OrthographicCamera;
  let camera: THREE.PerspectiveCamera | THREE.OrthographicCamera;
  let orbit: OrbitControls;
  let gizmo: TransformControls;
  let content = new THREE.Group();
  let handles = new Map<string, THREE.Mesh>();
  let lineMaterials: LineMaterial[] = [];
  let framed = false;
  let scanObject = $state.raw<THREE.Mesh | null>(null);
  /** Materials lit by the pointer hovering over their pipe or element. */
  let hovered: THREE.MeshStandardMaterial[] = [];
  let mounted = $state(false);

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
      if (o instanceof CSS2DObject) o.element.remove();
    });
  }

  function clear() {
    gizmo?.detach();
    dispose(content);
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
      const dimensioned = ui.allDimensions || r.id === selectedRoute;
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
        // Dimensions: every straight's length, every bend's angle and radius.
        if (!dimensioned) continue;
        if (piece.kind === 'straight') {
          const length = pts[0].distanceTo(pts[1]);
          if (length >= 20) label(mm(length, 0), pts[0].clone().lerp(pts[1], 0.5), 'dim');
        } else {
          label(`${piece.angle_deg.toFixed(0)}° R${shown(piece.radius_mm)} ${lengthUnit()}`, pts[Math.floor(pts.length / 2)], 'dim');
        }
      }
      // The flow solver's x-axis, drawn over the pipe.
      if (ui.centreline) {
        const geometry = new LineGeometry();
        geometry.setPositions(centreline.flatMap((p) => [p.x, p.y, p.z]));
        const lm = new LineMaterial({ color: 0xffd479, linewidth: 1.5, depthTest: false, transparent: true, opacity: 0.8 });
        lineMaterials.push(lm);
        const line = new Line2(geometry, lm);
        line.renderOrder = 2;
        content.add(line);
      }
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
      if (ui.labels) {
        const body = e.body[Math.floor(e.body.length / 2)];
        label(e.id, body ? v3(body.from).lerp(v3(body.to), 0.5) : v3(e.ports[0]?.position ?? [0, 0, 0]), 'tag');
      }
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
    const box = new THREE.Box3();
    content.traverse((o) => {
      if (o instanceof THREE.Mesh) box.expandByObject(o);
    });
    if (scanObject) box.union(new THREE.Box3().setFromObject(scanObject));
    return box;
  }

  /** Fits `box` tightly in view, looking from `from` (default: the current direction). */
  function fit(box: THREE.Box3, from?: readonly number[]) {
    if (box.isEmpty()) return;
    const back = from ? v3(from).normalize() : camera.position.clone().sub(orbit.target).normalize();
    const centre = box.getCenter(new THREE.Vector3());
    // The box's extent across and up the screen, and along the view, from its corners.
    const up = camera.up.clone().projectOnPlane(back);
    if (up.lengthSq() < 1e-9) up.set(1, 0, 0).projectOnPlane(back);
    up.normalize();
    const right = new THREE.Vector3().crossVectors(up, back).normalize();
    let [w, h, d] = [0, 0, 0];
    for (let i = 0; i < 8; i++) {
      const corner = new THREE.Vector3(i & 1 ? box.max.x : box.min.x, i & 2 ? box.max.y : box.min.y, i & 4 ? box.max.z : box.min.z).sub(centre);
      w = Math.max(w, Math.abs(corner.dot(right)));
      h = Math.max(h, Math.abs(corner.dot(up)));
      d = Math.max(d, Math.abs(corner.dot(back)));
    }
    [w, h] = [Math.max(w, 30) * 1.12, Math.max(h, 30) * 1.12];
    const aspect = host.clientWidth / Math.max(host.clientHeight, 1);
    const tan = Math.tan((perspective.fov * Math.PI) / 360);
    const distance = d + Math.max(h / tan, w / (tan * aspect));
    orbit.target.copy(centre);
    camera.position.copy(centre).addScaledVector(back, distance);
    for (const c of [perspective, orthographic]) {
      c.near = distance / 500;
      c.far = distance * 50;
    }
    // The orthographic view shows the same extent.
    const half = Math.max(h, w / aspect);
    Object.assign(orthographic, { top: half, bottom: -half, left: -half * aspect, right: half * aspect, zoom: 1 });
    perspective.updateProjectionMatrix();
    orthographic.updateProjectionMatrix();
    orbit.update();
  }

  /** Fits the whole system (and the scan) in view, from the isometric direction. */
  function frame() {
    fit(everything(), VIEWS[0][1]);
    framed = true;
  }

  /** Fits the selected pipe, bend point or element in view. */
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

  /** Switches between perspective and orthographic, keeping the view. */
  function project(ortho: boolean) {
    const next = ortho ? orthographic : perspective;
    if (camera === next) return;
    next.position.copy(camera.position);
    next.quaternion.copy(camera.quaternion);
    if (ortho) {
      // Match the perspective view's scale at the target.
      const aspect = host.clientWidth / Math.max(host.clientHeight, 1);
      const half = camera.position.distanceTo(orbit.target) * Math.tan((perspective.fov * Math.PI) / 360);
      Object.assign(orthographic, { top: half, bottom: -half, left: -half * aspect, right: half * aspect, zoom: 1 });
      orthographic.updateProjectionMatrix();
    }
    camera = next;
    orbit.object = next;
    gizmo.camera = next;
    // The typings leave out the camera the helper follows.
    (triad as unknown as { camera: THREE.Camera }).camera = next;
    orbit.update();
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
    const aspect = w / Math.max(h, 1);
    perspective.aspect = aspect;
    perspective.updateProjectionMatrix();
    const half = orthographic.top;
    Object.assign(orthographic, { left: -half * aspect, right: half * aspect });
    orthographic.updateProjectionMatrix();
    lineMaterials.forEach((m) => m.resolution.set(w, h));
  }

  // F fits everything; Esc drops the selection and any scan pick in progress.
  function keys(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.target instanceof HTMLElement && e.target.closest('input, select, textarea')) return;
    if (e.key === 'f' || e.key === 'F') fit(everything());
    else if (e.key === 'Escape') {
      app.selection = null;
      app.picking = null;
    }
  }

  /** A bend point dragged to a new place becomes an edit of its pipe. */
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
    scene.background = new THREE.Color(0x12161d);
    perspective = new THREE.PerspectiveCamera(35, 1, 5, 200000);
    orthographic = new THREE.OrthographicCamera(-1000, 1000, 1000, -1000, 1, 200000);
    for (const c of [perspective, orthographic]) c.up.set(0, 0, 1);
    camera = perspective;
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
    scene.add(new THREE.AxesHelper(250));
    gizmo = new TransformControls(camera, renderer.domElement);
    gizmo.setSize(0.8);
    gizmo.addEventListener('dragging-changed', (e) => {
      orbit.enabled = !e.value;
      if (!e.value) commitDrag();
    });
    scene.add(gizmo.getHelper());

    let down: { x: number; y: number } | null = null;
    const raycaster = new THREE.Raycaster();
    raycaster.firstHitOnly = true;
    const ndc = (e: PointerEvent) => {
      const rect = renderer.domElement.getBoundingClientRect();
      return new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    };
    renderer.domElement.addEventListener('pointerdown', (e) => (down = { x: e.clientX, y: e.clientY }));
    renderer.domElement.addEventListener('pointerup', (e) => {
      if (!down || gizmo.dragging || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) return;
      raycaster.setFromCamera(ndc(e), camera);
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
    renderer.domElement.addEventListener('pointermove', (e) => (pointer = ndc(e)));
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
    Object.assign(view, {
      fitAll: () => fit(everything()),
      fitSelection,
      look: (from: readonly number[]) => fit(everything(), from),
    });
    mounted = true;
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
    void [ui.allDimensions, ui.labels, ui.centreline, app.inches];
    if (mounted && layout) untrack(() => build(layout, selection));
  });

  $effect(() => {
    const ortho = ui.ortho;
    if (mounted) untrack(() => project(ortho));
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
    if (!mounted || !mesh) return;
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
    if (!mounted) return;
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
    if (mounted) renderer.domElement.style.cursor = app.picking !== null && app.scanMesh ? 'crosshair' : '';
  });

  $effect(() => {
    const ground = app.project?.ambient.ground_z_mm;
    if (!mounted || ground === undefined || !ui.grid) return;
    const grid = new THREE.GridHelper(8000, 80, 0x2f3949, 0x1f2631);
    grid.rotation.x = Math.PI / 2;
    grid.position.z = ground;
    scene.add(grid);
    return () => {
      scene.remove(grid);
      grid.geometry.dispose();
      (grid.material as THREE.Material).dispose();
    };
  });

  // The sweep, watched from the canvas: progress while it runs, the outcome when it ends.
  const td = $derived(app.timeDomain);
  const maxCycles = $derived(app.project?.solver.max_cycles ?? 1);
  const inFlight = $derived(Object.values(td.inFlight));
  const fraction = $derived(
    td.total ? (td.points.length + inFlight.reduce((sum, p) => sum + Math.min(p.cycle / maxCycles, 1), 0)) / td.total : 0,
  );
  let now = $state(Date.now());
  $effect(() => {
    if (!td.running) return;
    const tick = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(tick);
  });
  const clock = (ms: number) => {
    const s = Math.round(ms / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  };
  const elapsed = $derived(Math.max(0, now - td.startedAt));
  const left = $derived(fraction > 0.02 ? (elapsed * (1 - fraction)) / fraction : null);
  let dismissed = $state<object | null>(null);
  const outcome = $derived(td.result && dismissed !== td.result ? td.result : null);
  const drone = $derived(outcome?.metrics.drone_interior ?? outcome?.metrics.drone_exterior ?? null);

  const toggle = (key: 'allDimensions' | 'labels' | 'centreline' | 'grid') => {
    ui[key] = !ui[key];
    persist();
  };
</script>

<div class="viewport" bind:this={host} data-testid="viewport">
  <div class="hud views" role="toolbar" aria-label="View">
    <button onclick={() => fit(everything())} title="Fit everything (F)"><Icon name="fit" /></button>
    <button onclick={fitSelection} disabled={!app.selection} title="Fit the selection"><Icon name="target" /></button>
    <span class="sep"></span>
    {#each VIEWS as [name, from, title] (name)}
      <button class="text" onclick={() => fit(everything(), from)} {title}>{name}</button>
    {/each}
    <span class="sep"></span>
    <button
      class:on={ui.ortho}
      onclick={() => {
        ui.ortho = !ui.ortho;
        persist();
      }}
      title={ui.ortho ? 'Orthographic: switch to perspective' : 'Perspective: switch to orthographic'}
    >
      <Icon name={ui.ortho ? 'ortho' : 'persp'} />
    </button>
  </div>

  <div class="hud display" role="toolbar" aria-label="Display">
    <button class:on={ui.allDimensions} onclick={() => toggle('allDimensions')} title="Dimension every pipe (else only the selected one)">
      <Icon name="ruler" />
    </button>
    <button class:on={ui.labels} onclick={() => toggle('labels')} title="Element names"><Icon name="tag" /></button>
    <button class:on={ui.centreline} onclick={() => toggle('centreline')} title="The solver's centreline (its x-axis)">
      <Icon name="axis" />
    </button>
    <button class:on={ui.grid} onclick={() => toggle('grid')} title="Ground grid"><Icon name="grid" /></button>
    <span class="axes muted">X fwd · Y left · Z up · {lengthUnit()}</span>
  </div>

  {#if app.picking !== null}
    <div class="hud prompt">Click the scan to place reference point {app.picking + 1} · Esc to stop</div>
  {/if}

  {#if td.running}
    <div class="hud card" data-testid="sweep-monitor">
      <div class="card-head">
        <Icon name="activity" />
        <strong>Time-domain sweep</strong>
        <span class="muted mono">{td.points.length}/{td.total}</span>
      </div>
      <div class="bar"><span style:width="{(100 * fraction).toFixed(1)}%"></span></div>
      <div class="card-row mono">
        {inFlight.length} running · {clock(elapsed)} elapsed{#if left !== null} · ~{clock(left)} left{/if}
      </div>
      <button onclick={cancelTimeDomain}><Icon name="stop" size={13} /> Cancel</button>
    </div>
  {:else if outcome}
    <div class="hud card done" data-testid="sweep-result">
      <div class="card-head">
        <Icon name="activity" />
        <strong>Sweep complete</strong>
        <button class="close" onclick={() => (dismissed = outcome)} title="Dismiss"><Icon name="clear" size={12} /></button>
      </div>
      <div class="card-row mono">
        {outcome.points.length} speeds · {outcome.points.filter((p) => p.status === 'solved' && p.provenance.time_domain?.converged).length} converged
      </div>
      {#if drone}
        <div class="card-row">
          Drone peak <strong class="mono">{rpmText(drone.peak_rpm)}</strong> · <span class="mono">{drone.peak_db.toFixed(1)} dB</span>
          {outcome.metrics.drone_interior ? 'inside' : 'at the receiver'}
        </div>
      {/if}
      <div class="card-actions">
        <button class="primary" onclick={() => (show('listen'), play('current'))}><Icon name="speaker" size={13} /> Listen</button>
        <button onclick={() => show('overview')}>Results</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .viewport {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }

  .viewport :global(canvas) {
    display: block;
    width: 100%;
    height: 100%;
  }

  .hud {
    position: absolute;
    z-index: 3;
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 3px;
    border: 1px solid var(--line-2);
    border-radius: 6px;
    background: rgba(22, 26, 33, 0.88);
    backdrop-filter: blur(4px);
  }

  .hud button {
    display: flex;
    align-items: center;
    gap: 4px;
    min-height: 24px;
    padding: 2px 5px;
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .hud button:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--line-2);
  }

  .hud button.on {
    color: var(--accent);
    background: var(--accent-soft);
  }

  .hud button.text {
    font-size: 11.5px;
    padding: 2px 7px;
  }

  .views {
    top: 8px;
    right: 8px;
  }

  .display {
    bottom: 8px;
    left: 8px;
  }

  .axes {
    font-size: 11px;
    padding: 0 6px;
  }

  .sep {
    width: 1px;
    align-self: stretch;
    margin: 2px 3px;
    background: var(--line-2);
  }

  .prompt {
    top: 8px;
    left: 50%;
    transform: translateX(-50%);
    padding: 5px 12px;
    color: var(--warn);
  }

  .card {
    top: 8px;
    left: 8px;
    flex-direction: column;
    align-items: stretch;
    gap: 6px;
    width: 280px;
    padding: 9px 11px;
  }

  .card.done {
    border-color: #245236;
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--td);
  }

  .card.done .card-head {
    color: var(--good);
  }

  .card-head strong {
    color: var(--text);
  }

  .card-head .mono {
    margin-left: auto;
  }

  .close {
    margin-left: auto;
  }

  .card-row {
    font-size: 11.5px;
  }

  .card-actions {
    display: flex;
    gap: 6px;
  }

  .card button {
    align-self: flex-start;
    color: var(--text);
    border-color: var(--line-2);
    background: var(--panel-2);
  }

  .card button.primary {
    background: #1f4a80;
    border-color: #2d65a8;
  }

  .card .close {
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--bg);
    border: 1px solid var(--line-2);
    overflow: hidden;
  }

  .bar span {
    display: block;
    height: 100%;
    background: var(--td);
    transition: width 0.4s ease-out;
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
    border: 1px solid var(--line-2);
  }
</style>
