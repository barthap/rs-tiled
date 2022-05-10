use std::collections::HashMap;
use std::path::{Path, PathBuf};

use xml::attribute::OwnedAttribute;

use crate::error::{Error, Result};
use crate::image::Image;
use crate::properties::{parse_properties, Properties};
use crate::tile::TileData;
use crate::{util::*, Gid, ResourceCache, ResourceReader, Tile, TileId};

/// A collection of tiles for usage in maps and template objects.
///
/// Also see the [TMX docs](https://doc.mapeditor.org/en/stable/reference/tmx-map-format/#tileset).
#[derive(Debug, PartialEq, Clone)]
pub struct Tileset {
    /// The name of the tileset, set by the user.
    pub name: String,
    /// The (maximum) width in pixels of the tiles in this tileset. Irrelevant for [image collection]
    /// tilesets.
    ///
    /// [image collection]: Self::image
    pub tile_width: u32,
    /// The (maximum) height in pixels of the tiles in this tileset. Irrelevant for [image collection]
    /// tilesets.
    ///
    /// [image collection]: Self::image
    pub tile_height: u32,
    /// The spacing in pixels between the tiles in this tileset (applies to the tileset image).
    /// Irrelevant for image collection tilesets.
    pub spacing: u32,
    /// The margin around the tiles in this tileset (applies to the tileset image).
    /// Irrelevant for image collection tilesets.
    pub margin: u32,
    /// The number of tiles in this tileset. Note that tile IDs don't always have a connection with
    /// the tile count, and as such there may be tiles with an ID bigger than the tile count.
    pub tilecount: u32,
    /// The number of tile columns in the tileset. Editable for image collection tilesets, otherwise
    /// calculated using [image](Self::image) width, [tile width](Self::tile_width),
    /// [spacing](Self::spacing) and [margin](Self::margin).
    pub columns: u32,

    /// A tileset can either:
    /// * have a single spritesheet `image` in `tileset` ("regular" tileset);
    /// * have zero images in `tileset` and one `image` per `tile` ("image collection" tileset).
    ///
    /// --------
    /// - Source: [tiled issue #2117](https://github.com/mapeditor/tiled/issues/2117)
    /// - Source: [`columns` documentation](https://doc.mapeditor.org/en/stable/reference/tmx-map-format/#tileset)
    pub image: Option<Image>,

    /// All the tiles present in this tileset, indexed by their local IDs.
    tiles: HashMap<TileId, TileData>,

    /// The custom properties of the tileset.
    pub properties: Properties,
}

pub(crate) enum EmbeddedParseResultType {
    ExternalReference { tileset_path: PathBuf },
    Embedded { tileset: Tileset },
}

pub(crate) struct EmbeddedParseResult {
    pub first_gid: Gid,
    pub result_type: EmbeddedParseResultType,
}

/// Internal structure for holding mid-parse information.
struct TilesetProperties {
    spacing: Option<u32>,
    margin: Option<u32>,
    tilecount: u32,
    columns: Option<u32>,
    name: String,
    tile_width: u32,
    tile_height: u32,
    /// The root all non-absolute paths contained within the tileset are relative to.
    root_path: PathBuf,
}

impl Tileset {
    /// Gets the tile with the specified ID from the tileset.
    #[inline]
    pub fn get_tile(&self, id: TileId) -> Option<Tile> {
        self.tiles.get(&id).map(|data| Tile::new(self, data))
    }

    /// Iterates through the tiles from this tileset.
    #[inline]
    pub fn tiles(&self) -> impl ExactSizeIterator<Item = (TileId, Tile)> {
        self.tiles
            .iter()
            .map(move |(id, data)| (*id, Tile::new(self, data)))
    }
}

impl Tileset {
    pub(crate) fn parse_xml_in_map(
        parser: &mut impl Iterator<Item = XmlEventResult>,
        attrs: &[OwnedAttribute],
        path: &Path, // Template or Map file
        reader: &mut impl ResourceReader,
        cache: &mut impl ResourceCache,
    ) -> Result<EmbeddedParseResult> {
        Tileset::parse_xml_embedded(parser, attrs, path, reader, cache).or_else(|err| {
            if matches!(err, Error::MalformedAttributes(_)) {
                Tileset::parse_xml_reference(attrs, path)
            } else {
                Err(err)
            }
        })
    }

    fn parse_xml_embedded(
        parser: &mut impl Iterator<Item = XmlEventResult>,
        attrs: &[OwnedAttribute],
        path: &Path, // Template or Map file
        reader: &mut impl ResourceReader,
        cache: &mut impl ResourceCache,
    ) -> Result<EmbeddedParseResult> {
        let ((spacing, margin, columns, name), (tilecount, first_gid, tile_width, tile_height)) = get_attrs!(
           attrs,
           optionals: [
                ("spacing", spacing, |v:String| v.parse().ok()),
                ("margin", margin, |v:String| v.parse().ok()),
                ("columns", columns, |v:String| v.parse().ok()),
                ("name", name, Some),
            ],
           required: [
                ("tilecount", tilecount, |v:String| v.parse().ok()),
                ("firstgid", first_gid, |v:String| v.parse().ok().map(Gid)),
                ("tilewidth", width, |v:String| v.parse().ok()),
                ("tileheight", height, |v:String| v.parse().ok()),
            ],
            Error::MalformedAttributes("tileset must have a firstgid, tilecount, tilewidth, and tileheight with correct types".to_string())
        );

        let root_path = path.parent().ok_or(Error::PathIsNotFile)?.to_owned();

        Self::finish_parsing_xml(
            parser,
            TilesetProperties {
                spacing,
                margin,
                name: name.unwrap_or_default(),
                root_path,
                columns,
                tilecount,
                tile_height,
                tile_width,
            },
            reader,
            cache,
        )
        .map(|tileset| EmbeddedParseResult {
            first_gid,
            result_type: EmbeddedParseResultType::Embedded { tileset },
        })
    }

    fn parse_xml_reference(
        attrs: &[OwnedAttribute],
        map_path: &Path,
    ) -> Result<EmbeddedParseResult> {
        let (first_gid, source) = get_attrs!(
            attrs,
            required: [
                ("firstgid", first_gid, |v:String| v.parse().ok().map(Gid)),
                ("source", name, Some),
            ],
            Error::MalformedAttributes("Tileset reference must have a firstgid and source with correct types".to_string())
        );

        let tileset_path = map_path.parent().ok_or(Error::PathIsNotFile)?.join(source);

        Ok(EmbeddedParseResult {
            first_gid,
            result_type: EmbeddedParseResultType::ExternalReference { tileset_path },
        })
    }

    pub(crate) fn parse_external_tileset(
        parser: &mut impl Iterator<Item = XmlEventResult>,
        attrs: &[OwnedAttribute],
        path: &Path,
        reader: &mut impl ResourceReader,
        cache: &mut impl ResourceCache,
    ) -> Result<Tileset> {
        let ((spacing, margin, columns, name), (tilecount, tile_width, tile_height)) = get_attrs!(
            attrs,
            optionals: [
                ("spacing", spacing, |v:String| v.parse().ok()),
                ("margin", margin, |v:String| v.parse().ok()),
                ("columns", columns, |v:String| v.parse().ok()),
                ("name", name, Some),
            ],
            required: [
                ("tilecount", tilecount, |v:String| v.parse().ok()),
                ("tilewidth", width, |v:String| v.parse().ok()),
                ("tileheight", height, |v:String| v.parse().ok()),
            ],
            Error::MalformedAttributes("tileset must have a name, tile width and height with correct types".to_string())
        );

        let root_path = path.parent().ok_or(Error::PathIsNotFile)?.to_owned();

        Self::finish_parsing_xml(
            parser,
            TilesetProperties {
                spacing,
                margin,
                name: name.unwrap_or_default(),
                root_path,
                columns,
                tilecount,
                tile_height,
                tile_width,
            },
            reader,
            cache,
        )
    }

    fn finish_parsing_xml(
        parser: &mut impl Iterator<Item = XmlEventResult>,
        prop: TilesetProperties,
        reader: &mut impl ResourceReader,
        cache: &mut impl ResourceCache,
    ) -> Result<Tileset> {
        let mut image = Option::None;
        let mut tiles = HashMap::with_capacity(prop.tilecount as usize);
        let mut properties = HashMap::new();

        parse_tag!(parser, "tileset", {
            "image" => |attrs| {
                image = Some(Image::new(parser, attrs, &prop.root_path)?);
                Ok(())
            },
            "properties" => |_| {
                properties = parse_properties(parser)?;
                Ok(())
            },
            "tile" => |attrs| {
                let (id, tile) = TileData::new(parser, attrs, &prop.root_path, reader, cache)?;
                tiles.insert(id, tile);
                Ok(())
            },
        });

        // A tileset is considered an image collection tileset if there is no image attribute (because its tiles do).
        let is_image_collection_tileset = image.is_none();

        if !is_image_collection_tileset {
            for tile_id in 0..prop.tilecount {
                tiles.entry(tile_id).or_default();
            }
        }

        let margin = prop.margin.unwrap_or(0);
        let spacing = prop.spacing.unwrap_or(0);
        let columns = prop
            .columns
            .map(Ok)
            .unwrap_or_else(|| Self::calculate_columns(&image, prop.tile_width, margin, spacing))?;

        Ok(Tileset {
            name: prop.name,
            tile_width: prop.tile_width,
            tile_height: prop.tile_height,
            spacing,
            margin,
            columns,
            tilecount: prop.tilecount,
            image,
            tiles,
            properties,
        })
    }

    fn calculate_columns(
        image: &Option<Image>,
        tile_width: u32,
        margin: u32,
        spacing: u32,
    ) -> Result<u32> {
        image
            .as_ref()
            .map(|image| (image.width as u32 - margin + spacing) / (tile_width + spacing))
            .ok_or_else(|| {
                Error::MalformedAttributes(
                    "No <image> nor columns attribute in <tileset>".to_string(),
                )
            })
    }
}
