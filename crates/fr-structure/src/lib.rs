//! Linear elastic, small-displacement 2D Euler–Bernoulli frames by direct stiffness.
//! Units throughout: mm, N, N mm, radians; E is N/mm² (MPa). Joints share all
//! three DOFs (ux, uy, rz). No end releases, buckling, shear deformation or
//! connection design. This is mechanics, not a building-code compliance check.

pub const MAX_NODES: usize = 128;
pub const MAX_MEMBERS: usize = 256;
pub const SAMPLE_INTERVALS: usize = 32;

#[derive(Clone, Debug)]
pub struct Node {
    pub position: [f64; 2],
    pub restrained: [bool; 3],
    pub load: [f64; 3],
}

#[derive(Clone, Debug)]
pub struct Member {
    pub nodes: [usize; 2],
    pub e: f64,
    pub area: f64,
    pub inertia: f64,
    /// Uniform force per unit actual member length, in global frame axes.
    pub uniform_load: [f64; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub nodes: Vec<Node>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub fraction: f64,
    pub displacement: [f64; 2],
    pub axial_force: f64,
    pub shear_force: f64,
    pub moment: f64,
}

#[derive(Clone, Debug)]
pub struct MemberResult {
    /// Local forces exerted by the nodes on the member: Nx, Vy, Mz at each end.
    pub end_forces: [f64; 6],
    pub max_abs_axial: f64,
    pub max_abs_shear: f64,
    /// Evaluates the interior zero-shear location as well as both ends.
    pub max_abs_moment: f64,
    /// Includes the exact uniform-load particular solution, not only cubic
    /// interpolation of nodal DOFs (which misses sag in a fixed-fixed beam).
    pub samples: Vec<Sample>,
}

#[derive(Clone, Debug)]
pub struct Solution {
    pub displacements: Vec<[f64; 3]>,
    /// K u - f; approximately zero at free DOFs, support reactions at fixed DOFs.
    pub reactions: Vec<[f64; 3]>,
    pub members: Vec<MemberResult>,
}

type Matrix6 = [[f64; 6]; 6];

struct Element {
    length: f64,
    c: f64,
    s: f64,
    dofs: [usize; 6],
    k: Matrix6,
    t: Matrix6,
    equivalent: [f64; 6],
    q: [f64; 2],
}

impl Element {
    fn new(m: &Member, nodes: &[Node]) -> Result<Self, String> {
        if m.nodes[0] == m.nodes[1] || m.nodes.iter().any(|&n| n >= nodes.len()) {
            return Err("a member needs two distinct existing nodes".into());
        }
        if [m.e, m.area, m.inertia]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
            || m.uniform_load.iter().any(|v| !v.is_finite())
        {
            return Err(
                "member stiffness must be positive and finite; loads must be finite".into(),
            );
        }
        let a = nodes[m.nodes[0]].position;
        let b = nodes[m.nodes[1]].position;
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        if !length.is_finite() || length < 1e-6 {
            return Err("a member must have a finite length of at least 1e-6 mm".into());
        }
        let c = (b[0] - a[0]) / length;
        let s = (b[1] - a[1]) / length;
        let ea = m.e * m.area / length;
        let ei = m.e * m.inertia;
        let (a, b, d, e) = (
            12.0 * ei / length.powi(3),
            6.0 * ei / length.powi(2),
            4.0 * ei / length,
            2.0 * ei / length,
        );
        let k = [
            [ea, 0.0, 0.0, -ea, 0.0, 0.0],
            [0.0, a, b, 0.0, -a, b],
            [0.0, b, d, 0.0, -b, e],
            [-ea, 0.0, 0.0, ea, 0.0, 0.0],
            [0.0, -a, -b, 0.0, a, -b],
            [0.0, b, e, 0.0, -b, d],
        ];
        let mut t = [[0.0; 6]; 6];
        for i in [0, 3] {
            t[i][i] = c;
            t[i][i + 1] = s;
            t[i + 1][i] = -s;
            t[i + 1][i + 1] = c;
            t[i + 2][i + 2] = 1.0;
        }
        let [qx, qy] = [
            c * m.uniform_load[0] + s * m.uniform_load[1],
            -s * m.uniform_load[0] + c * m.uniform_load[1],
        ];
        let equivalent = [
            qx * length / 2.0,
            qy * length / 2.0,
            qy * length.powi(2) / 12.0,
            qx * length / 2.0,
            qy * length / 2.0,
            -qy * length.powi(2) / 12.0,
        ];
        if k.iter()
            .flatten()
            .chain(equivalent.iter())
            .any(|v| !v.is_finite())
        {
            return Err("member stiffness or load exceeds the numeric range".into());
        }
        Ok(Self {
            length,
            c,
            s,
            dofs: std::array::from_fn(|i| 3 * m.nodes[i / 3] + i % 3),
            k,
            t,
            equivalent,
            q: [qx, qy],
        })
    }

    fn result(&self, m: &Member, u: &[f64]) -> Result<MemberResult, String> {
        let local: [f64; 6] =
            std::array::from_fn(|i| (0..6).map(|j| self.t[i][j] * u[self.dofs[j]]).sum());
        let forces: [f64; 6] = std::array::from_fn(|i| {
            (0..6).map(|j| self.k[i][j] * local[j]).sum::<f64>() - self.equivalent[i]
        });
        let l = self.length;
        let [qx, qy] = self.q;
        let moment = |x: f64| -forces[2] + forces[1] * x + qy * x * x / 2.0;
        let mut max_m = moment(0.0).abs().max(moment(l).abs());
        if qy != 0.0 {
            let x = -forces[1] / qy;
            if x > 0.0 && x < l {
                max_m = max_m.max(moment(x).abs());
            }
        }
        let samples: Vec<_> = (0..=SAMPLE_INTERVALS)
            .map(|i| {
                let t = i as f64 / SAMPLE_INTERVALS as f64;
                let x = l * t;
                let axial =
                    (1.0 - t) * local[0] + t * local[3] + qx * x * (l - x) / (2.0 * m.e * m.area);
                let transverse = (1.0 - 3.0 * t * t + 2.0 * t.powi(3)) * local[1]
                    + l * (t - 2.0 * t * t + t.powi(3)) * local[2]
                    + (3.0 * t * t - 2.0 * t.powi(3)) * local[4]
                    + l * (-t * t + t.powi(3)) * local[5]
                    + qy * x.powi(2) * (l - x).powi(2) / (24.0 * m.e * m.inertia);
                Sample {
                    fraction: t,
                    displacement: [
                        self.c * axial - self.s * transverse,
                        self.s * axial + self.c * transverse,
                    ],
                    axial_force: -forces[0] - qx * x,
                    shear_force: forces[1] + qy * x,
                    moment: moment(x),
                }
            })
            .collect();
        if forces.iter().any(|v| !v.is_finite())
            || !max_m.is_finite()
            || samples.iter().any(|p| {
                p.displacement
                    .iter()
                    .chain([p.axial_force, p.shear_force, p.moment].iter())
                    .any(|v| !v.is_finite())
            })
        {
            return Err("analysis results exceed the numeric range".into());
        }
        Ok(MemberResult {
            end_forces: forces,
            max_abs_axial: forces[0].abs().max(forces[3].abs()),
            max_abs_shear: forces[1].abs().max(forces[4].abs()),
            max_abs_moment: max_m,
            samples,
        })
    }
}

impl Frame {
    pub fn solve(&self) -> Result<Solution, String> {
        if self.nodes.is_empty()
            || self.nodes.len() > MAX_NODES
            || self.members.is_empty()
            || self.members.len() > MAX_MEMBERS
        {
            return Err(format!(
                "a frame needs 1..={MAX_NODES} nodes and 1..={MAX_MEMBERS} members"
            ));
        }
        if self.nodes.iter().any(|n| {
            n.position
                .iter()
                .chain(n.load.iter())
                .any(|v| !v.is_finite())
        }) {
            return Err("node positions and loads must be finite".into());
        }
        let elements: Vec<_> = self
            .members
            .iter()
            .map(|m| Element::new(m, &self.nodes))
            .collect::<Result<_, _>>()?;
        if (0..self.nodes.len()).any(|i| !self.members.iter().any(|m| m.nodes.contains(&i))) {
            return Err("a frame contains an isolated node".into());
        }
        let n = 3 * self.nodes.len();
        let mut k = vec![vec![0.0; n]; n];
        let mut f: Vec<_> = self.nodes.iter().flat_map(|n| n.load).collect();
        for e in &elements {
            for i in 0..6 {
                f[e.dofs[i]] += (0..6).map(|a| e.t[a][i] * e.equivalent[a]).sum::<f64>();
                for j in 0..6 {
                    for a in 0..6 {
                        for b in 0..6 {
                            k[e.dofs[i]][e.dofs[j]] += e.t[a][i] * e.k[a][b] * e.t[b][j];
                        }
                    }
                }
            }
        }
        if k.iter().flatten().chain(f.iter()).any(|v| !v.is_finite()) {
            return Err("assembled stiffness or loads exceed the numeric range".into());
        }
        let free: Vec<_> = (0..n)
            .filter(|&i| !self.nodes[i / 3].restrained[i % 3])
            .collect();
        let mut u = vec![0.0; n];
        // Symmetric diagonal scaling avoids comparing rotational and translational
        // stiffness in different units. No artificial springs or regularization.
        let scale: Vec<_> = free.iter().map(|&i| k[i][i].sqrt()).collect();
        if scale.iter().any(|s| !s.is_finite() || *s <= 0.0) {
            return Err(unstable());
        }
        let r = free.len();
        let mut l = vec![vec![0.0; r]; r];
        for i in 0..r {
            for j in 0..=i {
                let value = k[free[i]][free[j]] / scale[i] / scale[j]
                    - (0..j).map(|a| l[i][a] * l[j][a]).sum::<f64>();
                if i == j {
                    if !value.is_finite() || value <= 1e-12 {
                        return Err(unstable());
                    }
                    l[i][j] = value.sqrt();
                } else {
                    l[i][j] = value / l[j][j];
                }
            }
        }
        let mut x = vec![0.0; r];
        for i in 0..r {
            x[i] = (f[free[i]] / scale[i] - (0..i).map(|j| l[i][j] * x[j]).sum::<f64>()) / l[i][i];
        }
        for i in (0..r).rev() {
            x[i] = (x[i] - (i + 1..r).map(|j| l[j][i] * x[j]).sum::<f64>()) / l[i][i];
        }
        for i in 0..r {
            u[free[i]] = x[i] / scale[i];
        }
        let reactions: Vec<_> = (0..n)
            .map(|i| (0..n).map(|j| k[i][j] * u[j]).sum::<f64>() - f[i])
            .collect();
        if u.iter().chain(reactions.iter()).any(|v| !v.is_finite()) {
            return Err(unstable());
        }
        for &i in &free {
            let magnitude = f[i].abs() + (0..n).map(|j| (k[i][j] * u[j]).abs()).sum::<f64>();
            if reactions[i].abs() > 1e-8 * magnitude.max(1.0) {
                return Err("the frame solution failed its equilibrium residual check".into());
            }
        }
        Ok(Solution {
            displacements: u.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect(),
            reactions: reactions
                .chunks_exact(3)
                .map(|v| [v[0], v[1], v[2]])
                .collect(),
            members: elements
                .iter()
                .zip(&self.members)
                .map(|(e, m)| e.result(m, &u))
                .collect::<Result<_, _>>()?,
        })
    }
}

fn unstable() -> String {
    "unstable or ill-conditioned frame: check supports, connections and stiffness ratios".into()
}

#[cfg(test)]
mod tests;
