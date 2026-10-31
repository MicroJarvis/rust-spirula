// spirula-shader: projection-tiles version: 1
// Three-buffer ABI v1. Every projected source occupies 16 u32 words.
struct Params { count:u32, stride:u32, tiles_x:u32, unused:u32 }
@group(0) @binding(0) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> src:array<u32>;
@group(0) @binding(2) var<storage,read_write> dst:array<u32>;
fn f(i:u32)->f32{return bitcast<f32>(src[i]);}
fn finite(v:f32)->bool{return abs(v)<=3.402823466e38;}
fn put(i:u32,v:f32){dst[i]=bitcast<u32>(v);}
fn row(i:u32)->vec3<f32>{return vec3<f32>(f(i),f(i+1u),f(i+2u));}
fn view(v:vec3<f32>)->vec3<f32>{return vec3<f32>(dot(row(0u),v),dot(row(3u),v),dot(row(6u),v));}
@compute @workgroup_size(64)
fn project(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if i>=p.count{return;}
 let o=i*16u;for(var k=0u;k<16u;k++){dst[o+k]=0u;}
 let s=32u+i*16u;
 let c=view(vec3<f32>(f(s),f(s+1u),f(s+2u))-row(9u));
 let depth=-c.z;if !finite(depth){dst[o+15u]=2u;return;}if depth<=0.0{return;}
 let opacity=1.0/(1.0+exp(-f(s+12u)));let cutoff=f(19u);
 if opacity<cutoff{return;}
 let fx=f(12u);let fy=f(13u);let cx=f(14u);let cy=f(15u);
 let w=f(16u);let h=f(17u);let tile=f(18u);
 let pixel=vec2<f32>(fx*c.x/depth+cx,-fy*c.y/depth+cy);
 let jx=vec3<f32>(fx/depth,0.0,clamp(fx*c.x/depth,-0.15*w-cx,1.15*w-cx)/depth);
 let jy=vec3<f32>(0.0,-fy/depth,clamp(-fy*c.y/depth,-0.15*h-cy,1.15*h-cy)/depth);
 let q=normalize(vec4<f32>(f(s+4u),f(s+5u),f(s+6u),f(s+7u)));
 let qw=q.x;let x=q.y;let y=q.z;let z=q.w;
 let axes=array<vec3<f32>,3>(vec3<f32>(1.0-2.0*(y*y+z*z),2.0*(x*y+qw*z),2.0*(x*z-qw*y)),
  vec3<f32>(2.0*(x*y-qw*z),1.0-2.0*(x*x+z*z),2.0*(y*z+qw*x)),
  vec3<f32>(2.0*(x*z+qw*y),2.0*(y*z-qw*x),1.0-2.0*(x*x+y*y)));
 var cov=vec3<f32>(0.3,0.0,0.3);
 for(var k=0u;k<3u;k++){
  let axis=view(axes[k])*exp(f(s+8u+k));let ax=dot(jx,axis);let ay=dot(jy,axis);
  cov+=vec3<f32>(ax*ax,ax*ay,ay*ay);
 }
 let extend=min(3.33,sqrt(2.0*log(opacity/cutoff)));
 let radius=extend*sqrt(vec2<f32>(cov.x,cov.z));let ray=max(depth,length(c));let det=cov.x*cov.z-cov.y*cov.y;
 if !finite(depth)||!finite(ray)||!finite(pixel.x)||!finite(pixel.y)||!finite(radius.x)||!finite(radius.y)
   ||!finite(cov.x)||!finite(cov.y)||!finite(cov.z)||!finite(det)||det<=0.0 {
  dst[o+15u]=2u;return;
 }
 let lo=pixel-radius;let hi=pixel+radius;
 if hi.x<=0.0||hi.y<=0.0||lo.x>=w||lo.y>=h{return;}
 let extent=vec2<u32>(u32(ceil(w/tile)),u32(ceil(h/tile)));
 let bmin=min(vec2<u32>(floor((max(lo,vec2<f32>(0.0))+0.5)/tile)),extent);
 let bmax=min(vec2<u32>(ceil((min(hi,vec2<f32>(w,h))+0.5)/tile)),extent);
 if any(bmin>=bmax){return;}
 put(o,pixel.x);put(o+1u,pixel.y);put(o+2u,depth);put(o+3u,ray);
 put(o+4u,radius.x);put(o+5u,radius.y);put(o+6u,cov.x);put(o+7u,cov.y);put(o+8u,cov.z);put(o+9u,opacity);
 dst[o+10u]=bmin.x;dst[o+11u]=bmin.y;dst[o+12u]=bmax.x;dst[o+13u]=bmax.y;
 dst[o+14u]=min((bmax.x-bmin.x)*(bmax.y-bmin.y),0x80000000u);dst[o+15u]=1u;
}
@compute @workgroup_size(64)
fn scan(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if i>=p.count{return;}
 let o=i*16u;for(var k=0u;k<16u;k++){dst[o+k]=src[o+k];}
 var a=src[o+14u];if i>=p.stride {
  let b=src[(i-p.stride)*16u+14u];
  // Saturation is an explicit rejected overflow marker, never a wrapped count.
  if a>=0x80000000u || b>=0x80000000u || a>0x80000000u-b {a=0x80000000u;}else{a+=b;}
 }
 dst[o+14u]=a;
}
@compute @workgroup_size(64)
fn emit(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if i>=p.count{return;}
 let o=i*16u;if src[o+15u]!=1u{return;}
 var at=0u;if i>0u{at=src[(i-1u)*16u+14u];}
 for(var y=src[o+11u];y<src[o+13u];y++){
  for(var x=src[o+10u];x<src[o+12u];x++){
   dst[at*2u]=y*p.tiles_x+x;dst[at*2u+1u]=i;at++;
  }
 }
}
