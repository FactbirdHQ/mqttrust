use crate::{Publish, encoding::Pid};


pub enum Error {

}


pub trait Spooler {
    /// Put the specified data into the persistent store.
    async fn add(&mut self, message: Publish<'_>) -> Result<(), Error>;

    /// Retrieve the specified data from the persistent store.
    async fn get_message_by_id(&mut self, pid: Pid) -> Result<Publish<'_>, Error>;

    /// Remove the data for the specified key from the store.
    async fn remove_message_by_id(&mut self, key: &str) -> Result<(), Error>;

    /// Returns the keys in this persistent data store.
    async fn get_all_message_ids(&self) -> Result<(), Error>;
}


// pub trait Spooler {
//     /// Put the specified data into the persistent store.
//     async fn put(&mut self, key: &str, data: &[u8]) -> Result<(), Error>;

//     /// Retrieve the specified data from the persistent store.
//     async fn get(&mut self, key: &str, buf: &mut [u8]) -> Result<usize, Error>;

//     /// Remove the data for the specified key from the store.
//     async fn remove(&mut self, key: &str) -> Result<(), Error>;

//     /// Returns the keys in this persistent data store.
//     async fn keys(&self) -> Result<(), Error>;

//     /// Clears the persistence store, so that it no longer contains any
//     /// persisted data.
//     async fn clear(&mut self) -> Result<(), Error>;

//     /// Returns whether any data has been persisted using the specified key.
//     async fn contains_key(&mut self, key: &str) -> Result<(), Error>;

// }

pub struct MemSpooler<const N: usize> {
    buf: [u8; N],
    len: usize
}

impl<const N: usize> Spooler for MemSpooler<N> {
    async fn add(&mut self, message: Publish<'_>) -> Result<(), Error> {
        todo!()
    }

    async fn get_message_by_id(&mut self, pid: Pid) -> Result<Publish<'_>, Error> {
        todo!()
    }

    async fn remove_message_by_id(&mut self, key: &str) -> Result<(), Error> {
        todo!()
    }

    async fn get_all_message_ids(&self) -> Result<(), Error> {
        todo!()
    }
}