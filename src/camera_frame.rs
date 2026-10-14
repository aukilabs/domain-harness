//! Generic binary VideoFrame payload; capture identity stays in the SDK envelope.
use anyhow::{Result, ensure};
use auki_components::{VideoFrame, PayloadContract, OutputManifest};
use auki_component_protocol::{ObservationEncoder, ObservationDecoder};
pub const SCHEMA: &str = "spatial-harness.video-frame-binary/v1";
pub const ENCODING: &str = "spatial-harness.video-frame-packed/v1";
pub const LEGACY_SCHEMA: &str = "auki.video-frame/v1";
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const BROWSER_MIME: &str = "application/vnd.spatial-harness.camera-frame";

pub fn supported(schema:&str)->bool {matches!(schema,SCHEMA|LEGACY_SCHEMA)}
pub fn validate(f:&VideoFrame)->Result<()> {
    ensure!(f.width>0 && f.height>0 && f.width<=4096 && f.height<=4096
        && !f.bytes.is_empty() && f.bytes.len()<=MAX_BYTES,"Camera frame exceeds limits");
    let pixels=f.width as usize*f.height as usize;
    match f.encoding.as_str() {
        "jpeg"|"jpg"=>{
            let reader=image::ImageReader::with_format(std::io::Cursor::new(f.bytes.as_ref()),image::ImageFormat::Jpeg);
            ensure!(reader.into_dimensions()?==(f.width,f.height),"JPEG dimensions differ from frame");
        },
        "rgb8"=>ensure!(f.bytes.len()==pixels*3,"Invalid RGB frame length"),
        "luma8"=>ensure!(f.bytes.len()==pixels,"Invalid grayscale frame length"),
        _=>anyhow::bail!("Unsupported camera encoding"),
    }
    Ok(())
}
pub fn validate_output(f:&VideoFrame,o:&OutputManifest)->Result<()> {
    validate(f)?;
    ensure!(matches!(&o.payload,PayloadContract::Camera(c) if supported(&c.schema)
        && c.width==f.width && c.height==f.height && c.encoding==f.encoding),"Camera frame differs from Output contract");
    Ok(())
}
pub fn pack(f:&VideoFrame)->Result<Vec<u8>> {
    validate(f)?;
    let tag=match f.encoding.as_str(){"jpeg"=>1,"jpg"=>2,"rgb8"=>3,"luma8"=>4,_=>unreachable!()};
    let mut bytes=Vec::with_capacity(16+f.bytes.len());
    bytes.extend_from_slice(b"ACF1");bytes.extend_from_slice(&f.width.to_le_bytes());bytes.extend_from_slice(&f.height.to_le_bytes());
    bytes.extend_from_slice(&[tag,0,0,0]);bytes.extend_from_slice(&f.bytes);Ok(bytes)
}
pub fn unpack(bytes:&[u8])->Result<VideoFrame> {
    ensure!(bytes.len()>16 && bytes.len()<=16+MAX_BYTES,"Invalid camera payload length");
    ensure!(&bytes[..4]==b"ACF1" && bytes[13..16]==[0,0,0],"Invalid camera header");
    let encoding=match bytes[12]{1=>"jpeg",2=>"jpg",3=>"rgb8",4=>"luma8",_=>anyhow::bail!("Unknown camera encoding")};
    let f=VideoFrame {width:u32::from_le_bytes(bytes[4..8].try_into().unwrap()),height:u32::from_le_bytes(bytes[8..12].try_into().unwrap()),encoding:encoding.into(),bytes:bytes[16..].into()};
    validate(&f)?;Ok(f)
}
pub fn encoder()->ObservationEncoder<VideoFrame> {
    ObservationEncoder::new(ENCODING,|f|pack(f).map_err(|e|e.to_string())).expect("static camera codec")
}
pub fn decoder(schema:&str)->Result<ObservationDecoder<VideoFrame>> {
    Ok(match schema {
        SCHEMA=>ObservationDecoder::new(ENCODING,|b|unpack(b).map_err(|e|e.to_string()))?,
        LEGACY_SCHEMA=>ObservationDecoder::new("application/json",|b|serde_json::from_slice(b).map_err(|e|e.to_string()))?,
        _=>anyhow::bail!("Unsupported camera schema"),
    })
}
/// Browser-only envelope: u32 LE metadata length, UTF-8 metadata, raw image.
/// Image bytes are never put into a JSON value or base64 string.
pub fn browser_packet(metadata:&serde_json::Value,image:&[u8])->Result<Vec<u8>> {
    let meta=serde_json::to_vec(metadata)?;
    ensure!(meta.len()<=1024*1024,"Camera overlay metadata exceeds 1 MiB limit ({} bytes)",meta.len());
    ensure!(image.len()<=MAX_BYTES,"Camera image exceeds 8 MiB limit ({} bytes)",image.len());
    let mut packet=Vec::with_capacity(4+meta.len()+image.len());
    packet.extend_from_slice(&(meta.len() as u32).to_le_bytes());packet.extend_from_slice(&meta);packet.extend_from_slice(image);Ok(packet)
}
