//! Portal mounting constraints in an established gravity-aligned, Y-up frame.
use anyhow::{ensure, Result};

#[derive(Debug, PartialEq, Eq)]
pub enum Mount { Floor, Wall }

fn dot(a:[f64;3],b:[f64;3])->f64 { a.iter().zip(b).map(|(a,b)|a*b).sum() }
fn cross(a:[f64;3],b:[f64;3])->[f64;3] {
    [a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]
}
fn multiply(a:[f64;4],b:[f64;4])->[f64;4] {
    let [w,x,y,z]=a;let [v,i,j,k]=b;
    [w*v-x*i-y*j-z*k,w*i+x*v+y*k-z*j,w*j-x*k+y*v+z*i,w*k+x*j-y*i+z*v]
}
fn axis(q:[f64;4],v:[f64;3])->[f64;3] {
    let r=multiply(multiply(q,[0.,v[0],v[1],v[2]]),[q[0],-q[1],-q[2],-q[3]]);
    [r[1],r[2],r[3]]
}

/// Portal +Z is its printed-face normal and +Y its top. Floor markers face
/// upward; wall markers have their printed top upward. Preserve heading and
/// translation; remove only the small tilt inconsistent with the chosen mount.
pub fn fit(q:[f64;4])->Result<(Mount,[f64;4])> {
    let length=q.iter().map(|v|v*v).sum::<f64>().sqrt();
    ensure!(q.iter().all(|v|v.is_finite()) && (length-1.).abs()<0.001,"Invalid Portal orientation");
    let q=q.map(|v|v/length);
    let up=[0.,1.,0.];let normal=axis(q,[0.,0.,1.]);let top=axis(q,[0.,1.,0.]);
    let floor_error=dot(normal,up).clamp(-1.,1.).acos();
    let wall_error=dot(top,up).clamp(-1.,1.).acos();
    let (mount,direction,error)=if floor_error<wall_error {(Mount::Floor,normal,floor_error)} else {(Mount::Wall,top,wall_error)};
    ensure!(error<=20_f64.to_radians(),"Portal does not fit a floor or upright wall within 20 degrees; observe again");
    let c=cross(direction,up);let correction=[1.+dot(direction,up),c[0],c[1],c[2]];
    let length=correction.iter().map(|v|v*v).sum::<f64>().sqrt();
    Ok((mount,multiply(correction.map(|v|v/length),q)))
}

/// This is an existing producer frame contract, not an inference from generic
/// axis labels. A boot-pose robot map can be tilted even when labelled Z-up.
pub fn native_is_gravity_aligned(frame:&str)->bool {
    frame.strip_prefix("phone-ar:").is_some_and(|s|uuid::Uuid::parse_str(s).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rx(degrees:f64)->[f64;4] {let h=degrees.to_radians()/2.;[h.cos(),h.sin(),0.,0.]}
    #[test] fn fits_floor_and_wall_and_preserves_heading()->Result<()> {
        let yaw=[0.8,0.,0.6,0.];
        for (q,expected,local_axis) in [(multiply(yaw,rx(-80.)),Mount::Floor,[0.,0.,1.]),(multiply(yaw,rx(10.)),Mount::Wall,[0.,1.,0.])] {
            let (mount,snapped)=fit(q)?;assert_eq!(mount,expected);
            assert!((dot(axis(snapped,local_axis),[0.,1.,0.])-1.).abs()<1e-10);
            assert!((dot(axis(snapped,[1.,0.,0.]),axis(q,[1.,0.,0.]))-1.).abs()<1e-10);
        } Ok(())
    }
    #[test] fn rejects_ambiguous_inverted_and_invalid() {
        for q in [rx(-45.),rx(90.),rx(180.),[0.;4],[f64::NAN,0.,0.,0.]] {assert!(fit(q).is_err());}
        assert!(!native_is_gravity_aligned("camera"));
        assert!(!native_is_gravity_aligned("bracketbot-local:any:zup"));
        assert!(native_is_gravity_aligned(&format!("phone-ar:{}",uuid::Uuid::new_v4())));
    }
}
