//! Addresses and actors of the spatial elements: `IfcPostalAddress` of a site or building, and
//! `IfcActor` with its `IfcActorRole` assigned to a spatial element by `IfcRelAssignsToActor`.

use arc_base::Result;

use super::{
    Parameter, Writer, choice, derived_id, enumeration, invalid, label, reference, references, role,
};

/// A postal address (`IfcPostalAddress`) of a site or building. At least one of the location
/// fields, lines, box, town, region, postal code or country is set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Address {
    /// The purpose, compared in upper case with `IfcAddressTypeEnum` (`OFFICE`, `SITE`, `HOME`,
    /// `DISTRIBUTIONPOINT`); another text is written as `USERDEFINED` with the text as the
    /// user-defined purpose.
    pub purpose: Option<String>,
    /// The `Description`.
    pub description: Option<String>,
    /// The `InternalLocation`.
    pub internal_location: Option<String>,
    /// The `AddressLines`.
    pub lines: Vec<String>,
    /// The `PostalBox`.
    pub postal_box: Option<String>,
    /// The `Town`.
    pub town: Option<String>,
    /// The `Region`.
    pub region: Option<String>,
    /// The `PostalCode`.
    pub postal_code: Option<String>,
    /// The `Country`.
    pub country: Option<String>,
}

const PURPOSES: &[&str] = &["OFFICE", "SITE", "HOME", "DISTRIBUTIONPOINT"];

/// The roles of `IfcRoleEnum`.
const ROLES: &[&str] = &[
    "SUPPLIER",
    "MANUFACTURER",
    "CONTRACTOR",
    "SUBCONTRACTOR",
    "ARCHITECT",
    "STRUCTURALENGINEER",
    "COSTENGINEER",
    "CLIENT",
    "BUILDINGOWNER",
    "BUILDINGOPERATOR",
    "MECHANICALENGINEER",
    "ELECTRICALENGINEER",
    "PROJECTMANAGER",
    "FACILITIESMANAGER",
    "CIVILENGINEER",
    "COMMISSIONINGENGINEER",
    "ENGINEER",
    "OWNER",
    "CONSULTANT",
    "CONSTRUCTIONMANAGER",
    "FIELDCONSTRUCTIONMANAGER",
    "RESELLER",
];

/// The party an actor stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Party {
    /// An organization (`IfcOrganization`) with its name.
    Organization(String),
    /// A person (`IfcPerson`); at least one of the names is set.
    Person {
        /// The `FamilyName`.
        family_name: Option<String>,
        /// The `GivenName`.
        given_name: Option<String>,
    },
}

/// An actor (`IfcActor`) assigned to a spatial element with a role (`IfcRelAssignsToActor`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Actor {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The name of the actor.
    pub name: String,
    /// The party.
    pub party: Party,
    /// The role, compared in upper case with `IfcRoleEnum` (`ARCHITECT`, `CLIENT`, `CONTRACTOR`,
    /// ...); another text is written as `USERDEFINED` with the text as the user-defined role;
    /// `None` assigns the actor without a role.
    pub role: Option<String>,
}

impl Actor {
    /// An actor named `name` for `party`, without a role.
    #[must_use]
    pub fn new(id: u128, name: &str, party: Party) -> Self {
        Self {
            id,
            name: name.to_owned(),
            party,
            role: None,
        }
    }
}

impl Writer<'_> {
    /// An `IfcPostalAddress`.
    pub(super) fn address(&mut self, address: &Address) -> Result<u64> {
        let optional = |t: &Option<String>| t.as_deref().map_or(Parameter::Omitted, label);
        let texts = [
            &address.internal_location,
            &address.postal_box,
            &address.town,
            &address.region,
            &address.postal_code,
            &address.country,
        ];
        if address.lines.is_empty() && texts.iter().all(|t| t.as_deref().is_none_or(str::is_empty))
        {
            return Err(invalid("address"));
        }
        let (purpose, user) = match address.purpose.as_deref() {
            None => (Parameter::Omitted, None),
            Some(_) => {
                let (p, u) = choice(address.purpose.as_deref(), PURPOSES);
                (p, u)
            }
        };
        let lines = if address.lines.is_empty() {
            Parameter::Omitted
        } else {
            Parameter::List(address.lines.iter().map(|l| label(l)).collect())
        };
        self.add(
            "IFCPOSTALADDRESS",
            &[
                purpose,
                optional(&address.description),
                user.unwrap_or(Parameter::Omitted),
                optional(&address.internal_location),
                lines,
                optional(&address.postal_box),
                optional(&address.town),
                optional(&address.region),
                optional(&address.postal_code),
                optional(&address.country),
            ],
        )
    }

    /// The actors of a spatial element: the party, the role, the `IfcActor` and one
    /// `IfcRelAssignsToActor` each.
    pub(super) fn actors(
        &mut self,
        owner: u128,
        actors: &[super::Actor],
        instance: u64,
    ) -> Result<()> {
        for (index, actor) in (0u32..).zip(actors) {
            let party = match &actor.party {
                Party::Organization(name) => {
                    if name.is_empty() {
                        return Err(invalid("actor"));
                    }
                    self.add(
                        "IFCORGANIZATION",
                        &[
                            Parameter::Omitted,
                            label(name),
                            Parameter::Omitted,
                            Parameter::Omitted,
                            Parameter::Omitted,
                        ],
                    )?
                }
                Party::Person {
                    family_name,
                    given_name,
                } => {
                    let named = |t: &Option<String>| t.as_deref().is_some_and(|t| !t.is_empty());
                    if !named(family_name) && !named(given_name) {
                        return Err(invalid("actor"));
                    }
                    let optional =
                        |t: &Option<String>| t.as_deref().map_or(Parameter::Omitted, label);
                    self.add(
                        "IFCPERSON",
                        &[
                            Parameter::Omitted,
                            optional(family_name),
                            optional(given_name),
                            Parameter::Omitted,
                            Parameter::Omitted,
                            Parameter::Omitted,
                            Parameter::Omitted,
                            Parameter::Omitted,
                        ],
                    )?
                }
            };
            let acting = match actor.role.as_deref() {
                Some(text) if !text.is_empty() => {
                    let upper = text.to_ascii_uppercase();
                    let (value, user) = if ROLES.contains(&upper.as_str()) {
                        (enumeration(&upper), Parameter::Omitted)
                    } else {
                        (enumeration("USERDEFINED"), label(text))
                    };
                    Some(self.add("IFCACTORROLE", &[value, user, Parameter::Omitted])?)
                }
                _ => None,
            };
            let id = self.global_id(actor.id)?;
            let actor_instance = self.add(
                "IFCACTOR",
                &[
                    id,
                    Parameter::Omitted,
                    label(&actor.name),
                    Parameter::Omitted,
                    Parameter::Omitted,
                    reference(party),
                ],
            )?;
            let rel = self.global_id(derived_id(owner, role::ACTOR, index))?;
            self.add(
                "IFCRELASSIGNSTOACTOR",
                &[
                    rel,
                    Parameter::Omitted,
                    Parameter::Omitted,
                    Parameter::Omitted,
                    references(&[instance]),
                    Parameter::Omitted,
                    reference(actor_instance),
                    acting.map_or(Parameter::Omitted, reference),
                ],
            )?;
        }
        Ok(())
    }
}
