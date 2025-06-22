pub mod charinfo;
pub mod expressions;
pub mod guild;
pub mod random;
pub mod roles;
pub mod users;

#[must_use]
pub fn commands() -> Vec<crate::Command> {
    {
        expressions::commands()
            .into_iter()
            .chain(random::commands())
            .chain(users::commands())
            .chain(guild::commands())
            .chain(charinfo::commands())
            .chain(roles::commands())
            .collect()
    }
}
