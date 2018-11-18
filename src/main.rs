extern crate iron;
#[macro_use] extern crate lazy_static;
extern crate logger;
extern crate mime;
extern crate mount;
extern crate router;
extern crate handlebars_iron;
extern crate params;
extern crate staticfile;
extern crate uuid;
extern crate env_logger;

use iron::prelude::*;
use iron::{AfterMiddleware};
use iron::status;
use iron::headers::ContentType;
use iron::mime::Mime;
use logger::Logger;
use router::{Router, NoRoute};
use handlebars_iron::{Template, HandlebarsEngine, DirectorySource};
use mount::Mount;
use staticfile::Static;
use std::collections::BTreeMap;
use std::io::Error;
use params::Params;
use params::Value;
use std::process::Command;
use std::fs;
use uuid::Uuid;


lazy_static! {
    static ref GDAL_VERSION: String = get_gdal_version();
}

struct TempFile {
    name: String,
    path: String,
    multiple_file: bool
}

const CONTENT_FAILED : &str = "<html><body><div><h1>Conversion failed</h1></div></body></html>";

const CONTENT_404 : &str = r#"<html>
    <body>
    <h1>Error 404</h1>
    <p>Ressource not found</p>
    </body>
</html>"#;

struct Custom404;

impl AfterMiddleware for Custom404 {
    fn catch(&self, _: &mut Request, err: IronError) -> IronResult<Response> {
        if err.error.is::<NoRoute>() {
            Ok(Response::with((status::NotFound, CONTENT_404)))
        } else {
            println!("{:?}", err);
            Err(err)
        }
    }
}

fn main() {
    // Logging:
    env_logger::init();
    let logger = Logger::new(None);

    // Templates:
    let mut hbse = HandlebarsEngine::new();
    hbse.add(Box::new(DirectorySource::new("./templates", ".hbs")));
    if hbse.reload().is_err() {
        panic!("Unable to build templates");
    }

    // Routes:
    let mut router = Router::new();
    router.get("/", welcome, "index");
    router.post("/convert", convert, "convert");

    // Static files:
    let mut mount = Mount::new();
    mount.mount("/static", Static::new("static/"));
    mount.mount("/", router);

    // Full chain:
    let mut chain = Chain::new(mount);
    chain.link(logger);
    chain.link_after(hbse);
    chain.link_after(Custom404);
    Iron::new(chain).http("localhost:3000").expect("Unable to launch server on localhost:3000");

    // Handle index page with handlebars:
    fn welcome(_: &mut Request) -> IronResult<Response> {
        let mut resp = Response::new();
        let mut data = BTreeMap::new();

        data.insert(String::from("version"), "0.0.1".to_string());
        data.insert(String::from("gdal_version"), (*GDAL_VERSION).to_string());
        resp.set_mut(Template::new("index", data)).set_mut(status::Ok);
        Ok(resp)
    }

    // Handle conversion requests:
    fn convert(req: &mut Request) -> IronResult<Response> {
        // Get the geographic layer:
        let f = &match get_uploaded_filename(req, "file") {
            Some(file_desc) => file_desc,
            None => {
                return Ok(Response::with((
                    ContentType::html().0, status::Ok, CONTENT_FAILED)));
            },
        };
        let (format, mime_type) = get_output_format(req);
        // Convert it to GML and send the content back to the user:
        match convert_to_gml(&f.path, &f.name, format.as_str()) {
            Ok(result) => {
                if f.multiple_file {
                    remove_files(&f.path).unwrap_or_else(|_|{
                        println!("Something went wrong while removing temporary files");
                    });
                }
                Ok(Response::with((mime_type, status::Ok, result)))
            },
            Err(_) => {
                Ok(Response::with((
                    ContentType::html().0, status::Ok, CONTENT_FAILED)))
            }
        }
    }
}

// Call ogr2ogr to make the conversion and fetch the result from stdout:
fn convert_to_gml(source_name: &str, layer_name: &str, format: &str) -> Result<String, Error> {
    let c = Command::new("ogr2ogr")
        .arg("-f").arg(format)
        .arg("-t_srs").arg("EPSG:4326")
        .arg("-nln").arg(layer_name)
        .arg("/dev/stdout")
        .arg(source_name)
        .output()
        .expect("Failed to execute ogr2ogr");
    if c.status.success() {
        Ok(String::from_utf8_lossy(&c.stdout).into_owned())
    } else {
        println!(
            "status: {} stderr: {} stdout: {}",
            c.status, String::from_utf8_lossy(&c.stderr), String::from_utf8_lossy(&c.stdout));
        Err(Error::last_os_error())
    }
}

// Return the path to the temporaty file to convert and it's original name:
fn handle_single_file(file: &params::File) -> Option<TempFile> {
    let file_name = file.filename.clone().unwrap();
    let parts = file_name.split('.').collect::<Vec<&str>>();
    Some(TempFile {
        path: file.path.to_str().unwrap().to_string(),
        name: parts[0].to_string(),
        multiple_file: false })

}

// Handle shapefile:
fn handle_multiple_files(files: &[params::Value]) -> Option<TempFile> {
    let mut is_shapefile = false;
    let (mut path, mut real_name) = (String::from(""), String::from(""));
    let random_name = format!("{}", Uuid::new_v4()).replace("-", "");
    for f in files {
        if let Value::File(ref file) = f {
            let file_name = file.filename.clone().unwrap();
            let parts = file_name.split('.').collect::<Vec<&str>>();
            let destination_file = &{ String::from("/tmp/") + &random_name + "." + parts[1]};
            // Use the same base name for each component of the Shapefile:
            mv_file(file.path.to_str().unwrap(), destination_file);
            // Only store the path of the .shp file:
            if parts.len() > 1 && parts[1] == "shp" {
                is_shapefile = true;
                path = destination_file.to_string();
                real_name = parts[0].to_string();
            }
        }
    }
    if is_shapefile {
        Some(TempFile { path: path, name: real_name, multiple_file: true })
    } else {
        None
    }
}

// Read the output format selected by the other, defaults to "GML" if invalid or non-specified:
fn get_output_format(req: &mut Request) -> (String, Mime) {
    match req.get_ref::<Params>().unwrap().find(&["output"]) {
        // Handle multiple files:
        Some(&Value::String(ref output_format)) => {
            match output_format.to_lowercase().as_str() {
                "geojson" => ("geojson".to_string(), "text/json".parse::<Mime>().unwrap()),
                "gml" | _ => ("GML".to_string(), "text/xml".parse::<Mime>().unwrap())
            }
        },
        _ => ("GML".to_string(), "text/xml".parse::<Mime>().unwrap())
    }
}

// Dispatch between reading one or multiple file, return the suitable path to be used by ogr2ogr:
fn get_uploaded_filename(req: &mut Request, param_name: &str) -> Option<TempFile> {
    match req.get_ref::<Params>().unwrap().find(&[param_name]) {
        Some(&Value::Array(ref files)) => {
            // Handle single file in an array:
            if files.len() == 1 {
                match files.get(0) {
                    Some(&Value::File(ref file)) => handle_single_file(file),
                    _ => None
                }
            // Handle multiple files in an array:
            } else { handle_multiple_files(files) }
        },
        // Handle single file:
        Some(&Value::File(ref file)) => {
            handle_single_file(file)
        },
        // No file:
        _ => {
            None
        },
    }
}

// Read once the gdal version information:
fn get_gdal_version() -> String {
    let output = Command::new("gdalinfo")
        .arg("--version")
        .output()
        .expect("Failed to execute gdalinfo");
    if !output.status.success() {
        String::from("")
    } else {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}


// Files have been moved if input was Shapefile; now delete them:
fn remove_files(path: &str) -> Result<(), Error> {
    fs::remove_file(path)?;
    fs::remove_file(path.replace("shp", "dbf"))?;
    fs::remove_file(path.replace("shp", "prj"))?;
    fs::remove_file(path.replace("shp", "shx"))?;
    fs::remove_file(path.replace("shp", "cpg"))?;
    Ok(())
}

// Wrapper arround mv command:
fn mv_file(source_name: &str, destination_name: &str) {
    let output = Command::new("mv")
        .arg(source_name)
        .arg(destination_name)
        .output()
        .expect("failed to execute process");
    if !output.status.success() {
        println!("status: {} stderr: {} stdout: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout));
    }
}
